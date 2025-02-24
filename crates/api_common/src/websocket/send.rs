use crate::{
  comment::CommentResponse,
  community::CommunityResponse,
  context::LemmyContext,
  post::PostResponse,
  private_message::PrivateMessageResponse,
  utils::{check_person_block, get_interface_language, send_email_to_user},
  websocket::OperationType,
};
use lemmy_db_schema::{
  newtypes::{CommentId, CommunityId, LocalUserId, PersonId, PostId, PrivateMessageId},
  source::{
    actor_language::CommunityLanguage,
    comment::Comment,
    comment_reply::{CommentReply, CommentReplyInsertForm},
    person::Person,
    person_mention::{PersonMention, PersonMentionInsertForm},
    post::Post,
  },
  traits::{Crud, DeleteableOrRemoveable},
  SubscribedType,
};
use lemmy_db_views::structs::{CommentView, LocalUserView, PostView, PrivateMessageView};
use lemmy_db_views_actor::structs::CommunityView;
use lemmy_utils::{error::LemmyError, utils::mention::MentionData, ConnectionId};

#[tracing::instrument(skip_all)]
pub async fn send_post_ws_message<OP: ToString + Send + OperationType + 'static>(
  post_id: PostId,
  op: OP,
  websocket_id: Option<ConnectionId>,
  person_id: Option<PersonId>,
  context: &LemmyContext,
) -> Result<PostResponse, LemmyError> {
  let post_view = PostView::read(context.pool(), post_id, person_id).await?;

  let res = PostResponse { post_view };

  context
    .chat_server()
    .send_post(&op, &res, websocket_id)
    .await?;

  Ok(res)
}

// TODO: in many call sites in apub crate, we are setting an empty vec for recipient_ids,
//       we should get the actual recipient actors from somewhere
#[tracing::instrument(skip_all)]
pub async fn send_comment_ws_message_simple<OP: ToString + Send + OperationType + 'static>(
  comment_id: CommentId,
  op: OP,
  context: &LemmyContext,
) -> Result<CommentResponse, LemmyError> {
  send_comment_ws_message(comment_id, op, None, None, None, vec![], context).await
}

#[tracing::instrument(skip_all)]
pub async fn send_comment_ws_message<OP: ToString + Send + OperationType + 'static>(
  comment_id: CommentId,
  op: OP,
  websocket_id: Option<ConnectionId>,
  form_id: Option<String>,
  person_id: Option<PersonId>,
  recipient_ids: Vec<LocalUserId>,
  context: &LemmyContext,
) -> Result<CommentResponse, LemmyError> {
  let mut view = CommentView::read(context.pool(), comment_id, person_id).await?;

  if view.comment.deleted || view.comment.removed {
    view.comment = view.comment.blank_out_deleted_or_removed_info();
  }

  let mut res = CommentResponse {
    comment_view: view,
    recipient_ids,
    // The sent out form id should be null
    form_id: None,
  };

  context
    .chat_server()
    .send_comment(&op, &res, websocket_id)
    .await?;

  // The recipient_ids should be empty for returns
  res.recipient_ids = Vec::new();
  res.form_id = form_id;

  Ok(res)
}

#[tracing::instrument(skip_all)]
pub async fn send_community_ws_message<OP: ToString + Send + OperationType + 'static>(
  community_id: CommunityId,
  op: OP,
  websocket_id: Option<ConnectionId>,
  person_id: Option<PersonId>,
  context: &LemmyContext,
) -> Result<CommunityResponse, LemmyError> {
  let community_view = CommunityView::read(context.pool(), community_id, person_id).await?;
  let discussion_languages = CommunityLanguage::read(context.pool(), community_id).await?;

  let mut res = CommunityResponse {
    community_view,
    discussion_languages,
  };

  // Strip out the person id and subscribed when sending to others
  res.community_view.subscribed = SubscribedType::NotSubscribed;

  context
    .chat_server()
    .send_community_room_message(&op, &res, res.community_view.community.id, websocket_id)
    .await?;

  Ok(res)
}

#[tracing::instrument(skip_all)]
pub async fn send_pm_ws_message<OP: ToString + Send + OperationType + 'static>(
  private_message_id: PrivateMessageId,
  op: OP,
  websocket_id: Option<ConnectionId>,
  context: &LemmyContext,
) -> Result<PrivateMessageResponse, LemmyError> {
  let mut view = PrivateMessageView::read(context.pool(), private_message_id).await?;

  // Blank out deleted or removed info
  if view.private_message.deleted {
    view.private_message = view.private_message.blank_out_deleted_or_removed_info();
  }

  let res = PrivateMessageResponse {
    private_message_view: view,
  };

  // Send notifications to the local recipient, if one exists
  if res.private_message_view.recipient.local {
    let recipient_id = res.private_message_view.recipient.id;
    let local_recipient = LocalUserView::read_person(context.pool(), recipient_id).await?;

    context
      .chat_server()
      .send_user_room_message(&op, &res, local_recipient.local_user.id, websocket_id)
      .await?;
  }

  Ok(res)
}

#[tracing::instrument(skip_all)]
pub async fn send_local_notifs(
  mentions: Vec<MentionData>,
  comment: &Comment,
  person: &Person,
  post: &Post,
  do_send_email: bool,
  context: &LemmyContext,
) -> Result<Vec<LocalUserId>, LemmyError> {
  let mut recipient_ids = Vec::new();
  let inbox_link = format!("{}/inbox", context.settings().get_protocol_and_hostname());

  // Send the local mentions
  for mention in mentions
    .iter()
    .filter(|m| m.is_local(&context.settings().hostname) && m.name.ne(&person.name))
    .collect::<Vec<&MentionData>>()
  {
    let mention_name = mention.name.clone();
    let user_view = LocalUserView::read_from_name(context.pool(), &mention_name).await;
    if let Ok(mention_user_view) = user_view {
      // TODO
      // At some point, make it so you can't tag the parent creator either
      // This can cause two notifications, one for reply and the other for mention
      recipient_ids.push(mention_user_view.local_user.id);

      let user_mention_form = PersonMentionInsertForm {
        recipient_id: mention_user_view.person.id,
        comment_id: comment.id,
        read: None,
      };

      // Allow this to fail softly, since comment edits might re-update or replace it
      // Let the uniqueness handle this fail
      PersonMention::create(context.pool(), &user_mention_form)
        .await
        .ok();

      // Send an email to those local users that have notifications on
      if do_send_email {
        let lang = get_interface_language(&mention_user_view);
        send_email_to_user(
          &mention_user_view,
          &lang.notification_mentioned_by_subject(&person.name),
          &lang.notification_mentioned_by_body(&comment.content, &inbox_link, &person.name),
          context.settings(),
        )
      }
    }
  }

  // Send comment_reply to the parent commenter / poster
  if let Some(parent_comment_id) = comment.parent_comment_id() {
    let parent_comment = Comment::read(context.pool(), parent_comment_id).await?;

    // Get the parent commenter local_user
    let parent_creator_id = parent_comment.creator_id;

    // Only add to recipients if that person isn't blocked
    let creator_blocked = check_person_block(person.id, parent_creator_id, context.pool())
      .await
      .is_err();

    // Don't send a notif to yourself
    if parent_comment.creator_id != person.id && !creator_blocked {
      let user_view = LocalUserView::read_person(context.pool(), parent_creator_id).await;
      if let Ok(parent_user_view) = user_view {
        recipient_ids.push(parent_user_view.local_user.id);

        let comment_reply_form = CommentReplyInsertForm {
          recipient_id: parent_user_view.person.id,
          comment_id: comment.id,
          read: None,
        };

        // Allow this to fail softly, since comment edits might re-update or replace it
        // Let the uniqueness handle this fail
        CommentReply::create(context.pool(), &comment_reply_form)
          .await
          .ok();

        if do_send_email {
          let lang = get_interface_language(&parent_user_view);
          send_email_to_user(
            &parent_user_view,
            &lang.notification_comment_reply_subject(&person.name),
            &lang.notification_comment_reply_body(&comment.content, &inbox_link, &person.name),
            context.settings(),
          )
        }
      }
    }
  } else {
    // If there's no parent, its the post creator
    // Only add to recipients if that person isn't blocked
    let creator_blocked = check_person_block(person.id, post.creator_id, context.pool())
      .await
      .is_err();

    if post.creator_id != person.id && !creator_blocked {
      let creator_id = post.creator_id;
      let parent_user = LocalUserView::read_person(context.pool(), creator_id).await;
      if let Ok(parent_user_view) = parent_user {
        recipient_ids.push(parent_user_view.local_user.id);

        let comment_reply_form = CommentReplyInsertForm {
          recipient_id: parent_user_view.person.id,
          comment_id: comment.id,
          read: None,
        };

        // Allow this to fail softly, since comment edits might re-update or replace it
        // Let the uniqueness handle this fail
        CommentReply::create(context.pool(), &comment_reply_form)
          .await
          .ok();

        if do_send_email {
          let lang = get_interface_language(&parent_user_view);
          send_email_to_user(
            &parent_user_view,
            &lang.notification_post_reply_subject(&person.name),
            &lang.notification_post_reply_body(&comment.content, &inbox_link, &person.name),
            context.settings(),
          )
        }
      }
    }
  }

  Ok(recipient_ids)
}
