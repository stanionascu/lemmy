use crate::{
  activities::{
    community::send_activity_in_community,
    send_lemmy_activity,
    verify_is_public,
    verify_mod_action,
    verify_person,
    verify_person_in_community,
  },
  activity_lists::AnnouncableActivities,
  local_instance,
  objects::{
    comment::ApubComment,
    community::ApubCommunity,
    person::ApubPerson,
    post::ApubPost,
    private_message::ApubPrivateMessage,
  },
  protocol::{
    activities::deletion::{delete::Delete, undo_delete::UndoDelete},
    InCommunity,
  },
  ActorType,
  SendActivity,
};
use activitypub_federation::{
  core::object_id::ObjectId,
  traits::{Actor, ApubObject},
  utils::verify_domains_match,
};
use activitystreams_kinds::public;
use lemmy_api_common::{
  comment::{CommentResponse, DeleteComment, RemoveComment},
  community::{CommunityResponse, DeleteCommunity, RemoveCommunity},
  context::LemmyContext,
  post::{DeletePost, PostResponse, RemovePost},
  private_message::{DeletePrivateMessage, PrivateMessageResponse},
  utils::get_local_user_view_from_jwt,
  websocket::{
    send::{
      send_comment_ws_message_simple,
      send_community_ws_message,
      send_pm_ws_message,
      send_post_ws_message,
    },
    UserOperationCrud,
  },
};
use lemmy_db_schema::{
  source::{
    comment::{Comment, CommentUpdateForm},
    community::{Community, CommunityUpdateForm},
    person::Person,
    post::{Post, PostUpdateForm},
    private_message::{PrivateMessage, PrivateMessageUpdateForm},
  },
  traits::Crud,
};
use lemmy_utils::error::LemmyError;
use std::ops::Deref;
use url::Url;

pub mod delete;
pub mod delete_user;
pub mod undo_delete;

#[async_trait::async_trait(?Send)]
impl SendActivity for DeletePost {
  type Response = PostResponse;

  async fn send_activity(
    request: &Self,
    response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    let community = Community::read(context.pool(), response.post_view.community.id).await?;
    let deletable = DeletableObjects::Post(response.post_view.post.clone().into());
    send_apub_delete_in_community(
      local_user_view.person,
      community,
      deletable,
      None,
      request.deleted,
      context,
    )
    .await
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for RemovePost {
  type Response = PostResponse;

  async fn send_activity(
    request: &Self,
    response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    let community = Community::read(context.pool(), response.post_view.community.id).await?;
    let deletable = DeletableObjects::Post(response.post_view.post.clone().into());
    send_apub_delete_in_community(
      local_user_view.person,
      community,
      deletable,
      request.reason.clone().or_else(|| Some(String::new())),
      request.removed,
      context,
    )
    .await
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for DeleteComment {
  type Response = CommentResponse;

  async fn send_activity(
    request: &Self,
    response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let community_id = response.comment_view.community.id;
    let community = Community::read(context.pool(), community_id).await?;
    let person = Person::read(context.pool(), response.comment_view.creator.id).await?;
    let deletable = DeletableObjects::Comment(response.comment_view.comment.clone().into());
    send_apub_delete_in_community(person, community, deletable, None, request.deleted, context)
      .await
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for RemoveComment {
  type Response = CommentResponse;

  async fn send_activity(
    request: &Self,
    response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    let comment = Comment::read(context.pool(), request.comment_id).await?;
    let community = Community::read(context.pool(), response.comment_view.community.id).await?;
    let deletable = DeletableObjects::Comment(comment.into());
    send_apub_delete_in_community(
      local_user_view.person,
      community,
      deletable,
      request.reason.clone().or_else(|| Some(String::new())),
      request.removed,
      context,
    )
    .await
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for DeletePrivateMessage {
  type Response = PrivateMessageResponse;

  async fn send_activity(
    request: &Self,
    response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    send_apub_delete_private_message(
      &local_user_view.person.into(),
      response.private_message_view.private_message.clone(),
      request.deleted,
      context,
    )
    .await
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for DeleteCommunity {
  type Response = CommunityResponse;

  async fn send_activity(
    request: &Self,
    _response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    let community = Community::read(context.pool(), request.community_id).await?;
    let deletable = DeletableObjects::Community(community.clone().into());
    send_apub_delete_in_community(
      local_user_view.person,
      community,
      deletable,
      None,
      request.deleted,
      context,
    )
    .await
  }
}

#[async_trait::async_trait(?Send)]
impl SendActivity for RemoveCommunity {
  type Response = CommunityResponse;

  async fn send_activity(
    request: &Self,
    _response: &Self::Response,
    context: &LemmyContext,
  ) -> Result<(), LemmyError> {
    let local_user_view =
      get_local_user_view_from_jwt(&request.auth, context.pool(), context.secret()).await?;
    let community = Community::read(context.pool(), request.community_id).await?;
    let deletable = DeletableObjects::Community(community.clone().into());
    send_apub_delete_in_community(
      local_user_view.person,
      community,
      deletable,
      request.reason.clone().or_else(|| Some(String::new())),
      request.removed,
      context,
    )
    .await
  }
}

/// Parameter `reason` being set indicates that this is a removal by a mod. If its unset, this
/// action was done by a normal user.
#[tracing::instrument(skip_all)]
async fn send_apub_delete_in_community(
  actor: Person,
  community: Community,
  object: DeletableObjects,
  reason: Option<String>,
  deleted: bool,
  context: &LemmyContext,
) -> Result<(), LemmyError> {
  let actor = ApubPerson::from(actor);
  let is_mod_action = reason.is_some();
  let activity = if deleted {
    let delete = Delete::new(&actor, object, public(), Some(&community), reason, context)?;
    AnnouncableActivities::Delete(delete)
  } else {
    let undo = UndoDelete::new(&actor, object, public(), Some(&community), reason, context)?;
    AnnouncableActivities::UndoDelete(undo)
  };
  send_activity_in_community(
    activity,
    &actor,
    &community.into(),
    vec![],
    is_mod_action,
    context,
  )
  .await
}

#[tracing::instrument(skip_all)]
async fn send_apub_delete_private_message(
  actor: &ApubPerson,
  pm: PrivateMessage,
  deleted: bool,
  context: &LemmyContext,
) -> Result<(), LemmyError> {
  let recipient_id = pm.recipient_id;
  let recipient: ApubPerson = Person::read(context.pool(), recipient_id).await?.into();

  let deletable = DeletableObjects::PrivateMessage(pm.into());
  let inbox = vec![recipient.shared_inbox_or_inbox()];
  if deleted {
    let delete = Delete::new(actor, deletable, recipient.actor_id(), None, None, context)?;
    send_lemmy_activity(context, delete, actor, inbox, true).await?;
  } else {
    let undo = UndoDelete::new(actor, deletable, recipient.actor_id(), None, None, context)?;
    send_lemmy_activity(context, undo, actor, inbox, true).await?;
  };
  Ok(())
}

pub enum DeletableObjects {
  Community(ApubCommunity),
  Comment(ApubComment),
  Post(ApubPost),
  PrivateMessage(ApubPrivateMessage),
}

impl DeletableObjects {
  #[tracing::instrument(skip_all)]
  pub(crate) async fn read_from_db(
    ap_id: &Url,
    context: &LemmyContext,
  ) -> Result<DeletableObjects, LemmyError> {
    if let Some(c) = ApubCommunity::read_from_apub_id(ap_id.clone(), context).await? {
      return Ok(DeletableObjects::Community(c));
    }
    if let Some(p) = ApubPost::read_from_apub_id(ap_id.clone(), context).await? {
      return Ok(DeletableObjects::Post(p));
    }
    if let Some(c) = ApubComment::read_from_apub_id(ap_id.clone(), context).await? {
      return Ok(DeletableObjects::Comment(c));
    }
    if let Some(p) = ApubPrivateMessage::read_from_apub_id(ap_id.clone(), context).await? {
      return Ok(DeletableObjects::PrivateMessage(p));
    }
    Err(diesel::NotFound.into())
  }

  pub(crate) fn id(&self) -> Url {
    match self {
      DeletableObjects::Community(c) => c.actor_id(),
      DeletableObjects::Comment(c) => c.ap_id.clone().into(),
      DeletableObjects::Post(p) => p.ap_id.clone().into(),
      DeletableObjects::PrivateMessage(p) => p.ap_id.clone().into(),
    }
  }
}

#[tracing::instrument(skip_all)]
pub(in crate::activities) async fn verify_delete_activity(
  activity: &Delete,
  is_mod_action: bool,
  context: &LemmyContext,
  request_counter: &mut i32,
) -> Result<(), LemmyError> {
  let object = DeletableObjects::read_from_db(activity.object.id(), context).await?;
  match object {
    DeletableObjects::Community(community) => {
      verify_is_public(&activity.to, &[])?;
      if community.local {
        // can only do this check for local community, in remote case it would try to fetch the
        // deleted community (which fails)
        verify_person_in_community(&activity.actor, &community, context, request_counter).await?;
      }
      // community deletion is always a mod (or admin) action
      verify_mod_action(
        &activity.actor,
        activity.object.id(),
        community.id,
        context,
        request_counter,
      )
      .await?;
    }
    DeletableObjects::Post(p) => {
      verify_is_public(&activity.to, &[])?;
      verify_delete_post_or_comment(
        &activity.actor,
        &p.ap_id.clone().into(),
        &activity.community(context, request_counter).await?,
        is_mod_action,
        context,
        request_counter,
      )
      .await?;
    }
    DeletableObjects::Comment(c) => {
      verify_is_public(&activity.to, &[])?;
      verify_delete_post_or_comment(
        &activity.actor,
        &c.ap_id.clone().into(),
        &activity.community(context, request_counter).await?,
        is_mod_action,
        context,
        request_counter,
      )
      .await?;
    }
    DeletableObjects::PrivateMessage(_) => {
      verify_person(&activity.actor, context, request_counter).await?;
      verify_domains_match(activity.actor.inner(), activity.object.id())?;
    }
  }
  Ok(())
}

#[tracing::instrument(skip_all)]
async fn verify_delete_post_or_comment(
  actor: &ObjectId<ApubPerson>,
  object_id: &Url,
  community: &ApubCommunity,
  is_mod_action: bool,
  context: &LemmyContext,
  request_counter: &mut i32,
) -> Result<(), LemmyError> {
  verify_person_in_community(actor, community, context, request_counter).await?;
  if is_mod_action {
    verify_mod_action(actor, object_id, community.id, context, request_counter).await?;
  } else {
    // domain of post ap_id and post.creator ap_id are identical, so we just check the former
    verify_domains_match(actor.inner(), object_id)?;
  }
  Ok(())
}

/// Write deletion or restoring of an object to the database, and send websocket message.
#[tracing::instrument(skip_all)]
async fn receive_delete_action(
  object: &Url,
  actor: &ObjectId<ApubPerson>,
  deleted: bool,
  context: &LemmyContext,
  request_counter: &mut i32,
) -> Result<(), LemmyError> {
  match DeletableObjects::read_from_db(object, context).await? {
    DeletableObjects::Community(community) => {
      if community.local {
        let mod_: Person = actor
          .dereference(context, local_instance(context).await, request_counter)
          .await?
          .deref()
          .clone();
        let object = DeletableObjects::Community(community.clone());
        let c: Community = community.deref().deref().clone();
        send_apub_delete_in_community(mod_, c, object, None, true, context).await?;
      }

      let community = Community::update(
        context.pool(),
        community.id,
        &CommunityUpdateForm::builder()
          .deleted(Some(deleted))
          .build(),
      )
      .await?;
      send_community_ws_message(
        community.id,
        UserOperationCrud::DeleteCommunity,
        None,
        None,
        context,
      )
      .await?;
    }
    DeletableObjects::Post(post) => {
      if deleted != post.deleted {
        let deleted_post = Post::update(
          context.pool(),
          post.id,
          &PostUpdateForm::builder().deleted(Some(deleted)).build(),
        )
        .await?;
        send_post_ws_message(
          deleted_post.id,
          UserOperationCrud::DeletePost,
          None,
          None,
          context,
        )
        .await?;
      }
    }
    DeletableObjects::Comment(comment) => {
      if deleted != comment.deleted {
        let deleted_comment = Comment::update(
          context.pool(),
          comment.id,
          &CommentUpdateForm::builder().deleted(Some(deleted)).build(),
        )
        .await?;
        send_comment_ws_message_simple(
          deleted_comment.id,
          UserOperationCrud::DeleteComment,
          context,
        )
        .await?;
      }
    }
    DeletableObjects::PrivateMessage(pm) => {
      let deleted_private_message = PrivateMessage::update(
        context.pool(),
        pm.id,
        &PrivateMessageUpdateForm::builder()
          .deleted(Some(deleted))
          .build(),
      )
      .await?;

      send_pm_ws_message(
        deleted_private_message.id,
        UserOperationCrud::DeletePrivateMessage,
        None,
        context,
      )
      .await?;
    }
  }
  Ok(())
}
