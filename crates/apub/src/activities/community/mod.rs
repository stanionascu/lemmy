use crate::{
  activities::send_lemmy_activity,
  activity_lists::AnnouncableActivities,
  objects::{community::ApubCommunity, person::ApubPerson},
  protocol::activities::community::announce::AnnounceActivity,
};
use activitypub_federation::traits::Actor;
use lemmy_api_common::context::LemmyContext;
use lemmy_db_schema::source::person::PersonFollower;
use lemmy_utils::error::LemmyError;
use url::Url;

pub mod announce;
pub mod collection_add;
pub mod collection_remove;
pub mod lock_page;
pub mod report;
pub mod update;

/// This function sends all activities which are happening in a community to the right inboxes.
/// For example Create/Page, Add/Mod etc, but not private messages.
///
/// Activities are sent to the community itself if it lives on another instance. If the community
/// is local, the activity is directly wrapped into Announce and sent to community followers.
/// Activities are also sent to those who follow the actor (with exception of moderation activities).
///
/// * `activity` - The activity which is being sent
/// * `actor` - The user who is sending the activity
/// * `community` - Community inside which the activity is sent
/// * `inboxes` - Any additional inboxes the activity should be sent to (for example,
///               to the user who is being promoted to moderator)
/// * `is_mod_activity` - True for things like Add/Mod, these are not sent to user followers
pub(crate) async fn send_activity_in_community(
  activity: AnnouncableActivities,
  actor: &ApubPerson,
  community: &ApubCommunity,
  extra_inboxes: Vec<Url>,
  is_mod_action: bool,
  context: &LemmyContext,
) -> Result<(), LemmyError> {
  // send to extra_inboxes
  send_lemmy_activity(context, activity.clone(), actor, extra_inboxes, false).await?;

  if community.local {
    // send directly to community followers
    AnnounceActivity::send(activity.clone().try_into()?, community, context).await?;
  } else {
    // send to the community, which will then forward to followers
    let inbox = vec![community.shared_inbox_or_inbox()];
    send_lemmy_activity(context, activity.clone(), actor, inbox, false).await?;
  }

  // send to those who follow `actor`
  if !is_mod_action {
    let inboxes = PersonFollower::list_followers(context.pool(), actor.id)
      .await?
      .into_iter()
      .map(|p| ApubPerson(p).shared_inbox_or_inbox())
      .collect();
    send_lemmy_activity(context, activity, actor, inboxes, false).await?;
  }

  Ok(())
}
