use crate::{
  activities::{
    community::send_activity_in_community,
    generate_activity_id,
    verify_is_public,
    verify_mod_action,
    verify_person_in_community,
  },
  activity_lists::AnnouncableActivities,
  local_instance,
  objects::{community::ApubCommunity, person::ApubPerson, post::ApubPost},
  protocol::{
    activities::{
      community::{collection_add::CollectionAdd, collection_remove::CollectionRemove},
      create_or_update::page::CreateOrUpdatePage,
      CreateOrUpdateType,
    },
    InCommunity,
  },
  ActorType,
  SendActivity,
};
use activitypub_federation::{
  core::object_id::ObjectId,
  data::Data,
  traits::{ActivityHandler, Actor},
};
use activitystreams_kinds::{activity::AddType, public};
use lemmy_api_common::{
  community::{AddModToCommunity, AddModToCommunityResponse},
  context::LemmyContext,
  post::{FeaturePost, PostResponse},
  utils::{generate_featured_url, generate_moderators_url, get_local_user_view_from_jwt},
};
use lemmy_db_schema::{
  impls::community::CollectionType,
  source::{
    community::{Community, CommunityModerator, CommunityModeratorForm},
    moderator::{ModAddCommunity, ModAddCommunityForm},
    person::Person,
    post::{Post, PostUpdateForm},
  },
  traits::{Crud, Joinable},
};
use lemmy_utils::error::LemmyError;
use url::Url;

impl CollectionAdd {
  #[tracing::instrument(skip_all)]
  pub async fn send_add_mod(
    community: &ApubCommunity,
    added_mod: &ApubPerson,
    actor: &ApubPerson,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let id = generate_activity_id(
      AddType::Add,
      &context.settings().get_protocol_and_hostname(),
    )?;
    let add = CollectionAdd {
      actor: ObjectId::new(actor.actor_id()),
      to: vec![public()],
      object: added_mod.actor_id(),
      target: generate_moderators_url(&community.actor_id)?.into(),
      cc: vec![community.actor_id()],
      kind: AddType::Add,
      id: id.clone(),
      audience: Some(ObjectId::new(community.actor_id())),
    };

    let activity = AnnouncableActivities::CollectionAdd(add);
    let inboxes = vec![added_mod.shared_inbox_or_inbox()];
    send_activity_in_community(activity, actor, community, inboxes, true, context).await
  }

  pub async fn send_add_featured_post(
    community: &ApubCommunity,
    featured_post: &ApubPost,
    actor: &ApubPerson,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let id = generate_activity_id(
      AddType::Add,
      &context.settings().get_protocol_and_hostname(),
    )?;
    let add = CollectionAdd {
      actor: ObjectId::new(actor.actor_id()),
      to: vec![public()],
      object: featured_post.ap_id.clone().into(),
      target: generate_featured_url(&community.actor_id)?.into(),
      cc: vec![community.actor_id()],
      kind: AddType::Add,
      id: id.clone(),
      audience: Some(ObjectId::new(community.actor_id())),
    };
    let activity = AnnouncableActivities::CollectionAdd(add);
    send_activity_in_community(activity, actor, community, vec![], true, context).await
  }
}

#[async_trait::async_trait(?Send)]
impl ActivityHandler for CollectionAdd {
  type DataType = LemmyContext;
  type Error = LemmyError;

  fn id(&self) -> &Url {
    &self.id
  }

  fn actor(&self) -> &Url {
    self.actor.inner()
  }

  #[tracing::instrument(skip_all)]
  async fn verify(
    &self,
    context: &Data<LemmyContext>,
    request_counter: &mut i32,
  ) -> Result<(), LemmyError> {
    verify_is_public(&self.to, &self.cc)?;
    let community = self.community(context, request_counter).await?;
    verify_person_in_community(&self.actor, &community, context, request_counter).await?;
    verify_mod_action(
      &self.actor,
      &self.object,
      community.id,
      context,
      request_counter,
    )
    .await?;
    Ok(())
  }

  #[tracing::instrument(skip_all)]
  async fn receive(
    self,
    context: &Data<LemmyContext>,
    request_counter: &mut i32,
  ) -> Result<(), LemmyError> {
    let (community, collection_type) =
      Community::get_by_collection_url(context.pool(), &self.target.into()).await?;
    match collection_type {
      CollectionType::Moderators => {
        let new_mod = ObjectId::<ApubPerson>::new(self.object)
          .dereference(context, local_instance(context).await, request_counter)
          .await?;

        // If we had to refetch the community while parsing the activity, then the new mod has already
        // been added. Skip it here as it would result in a duplicate key error.
        let new_mod_id = new_mod.id;
        let moderated_communities =
          CommunityModerator::get_person_moderated_communities(context.pool(), new_mod_id).await?;
        if !moderated_communities.contains(&community.id) {
          let form = CommunityModeratorForm {
            community_id: community.id,
            person_id: new_mod.id,
          };
          CommunityModerator::join(context.pool(), &form).await?;

          // write mod log
          let actor = self
            .actor
            .dereference(context, local_instance(context).await, request_counter)
            .await?;
          let form = ModAddCommunityForm {
            mod_person_id: actor.id,
            other_person_id: new_mod.id,
            community_id: community.id,
            removed: Some(false),
          };
          ModAddCommunity::create(context.pool(), &form).await?;
        }
        // TODO: send websocket notification about added mod
      }
      CollectionType::Featured => {
        let post = ObjectId::<ApubPost>::new(self.object)
          .dereference(context, local_instance(context).await, request_counter)
          .await?;
        let form = PostUpdateForm::builder()
          .featured_community(Some(true))
          .build();
        Post::update(context.pool(), post.id, &form).await?;
      }
    }
    Ok(())
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for AddModToCommunity {
  type Response = AddModToCommunityResponse;

  async fn send_activity(
    request: &Self,
    _response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    let community: ApubCommunity = Community::read(context.pool(), request.community_id)
      .await?
      .into();
    let updated_mod: ApubPerson = Person::read(context.pool(), request.person_id)
      .await?
      .into();
    if request.added {
      CollectionAdd::send_add_mod(
        &community,
        &updated_mod,
        &local_user_view.person.into(),
        context,
      )
      .await
    } else {
      CollectionRemove::send_remove_mod(
        &community,
        &updated_mod,
        &local_user_view.person.into(),
        context,
      )
      .await
    }
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for FeaturePost {
  type Response = PostResponse;

  async fn send_activity(
    request: &Self,
    response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    // Deprecated, for backwards compatibility with 0.17
    CreateOrUpdatePage::send(
      &response.post_view.post,
      local_user_view.person.id,
      CreateOrUpdateType::Update,
      context,
    )
    .await?;
    let community = Community::read(context.pool(), response.post_view.community.id)
      .await?
      .into();
    let post = response.post_view.post.clone().into();
    let person = local_user_view.person.into();
    if request.featured {
      CollectionAdd::send_add_featured_post(&community, &post, &person, context).await
    } else {
      CollectionRemove::send_remove_featured_post(&community, &post, &person, context).await
    }
  }
}
