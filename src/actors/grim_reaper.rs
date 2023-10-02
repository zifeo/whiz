use std::collections::{HashMap, HashSet};

use actix::prelude::*;
use subprocess::ExitStatus;

pub struct GrimReaperActor {
    live_invites: HashSet<String>,
    non_zero_deaths: HashMap<String, ExitStatus>,
}

impl GrimReaperActor {
    pub async fn start_new<T>(targets: HashMap<String, Addr<T>>) -> anyhow::Result<()>
    where
        T: Actor + Handler<PermaDeathInvite>,
        <T as actix::Actor>::Context: actix::dev::ToEnvelope<T, PermaDeathInvite>,
    {
        let reaper_addr = GrimReaperActor {
            live_invites: targets.keys().cloned().collect(),
            non_zero_deaths: Default::default(),
        }
        .start();
        for target in targets.values() {
            target
                .send(PermaDeathInvite {
                    reaper_addr: reaper_addr.clone(),
                })
                .await?;
        }
        Ok(())
    }
}

impl Actor for GrimReaperActor {
    type Context = Context<Self>;
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PermaDeathInvite {
    reaper_addr: Addr<crate::actors::grim_reaper::GrimReaperActor>,
}

impl PermaDeathInvite {
    pub fn rsvp(self, actor_name: String, exit_status: ExitStatus) {
        // FIXME: `do_send` might fail if actor "mailbox" is full
        self.reaper_addr.do_send(InviteAccepted {
            actor_name,
            exit_status,
        });
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct InviteAccepted {
    actor_name: String,
    exit_status: ExitStatus,
}

impl Handler<InviteAccepted> for GrimReaperActor {
    type Result = ();

    fn handle(&mut self, evt: InviteAccepted, _: &mut Context<Self>) -> Self::Result {
        assert!(self.live_invites.remove(&evt.actor_name));
        if !evt.exit_status.success() {
            self.non_zero_deaths.insert(evt.actor_name, evt.exit_status);
        }
        if self.live_invites.is_empty() {
            if let Some((_op_name, status)) = self.non_zero_deaths.iter().next() {
                // exit with the error code of the first aberrant task
                let code = match *status {
                    ExitStatus::Exited(code) => code as i32,
                    ExitStatus::Other(code) => code,
                    ExitStatus::Signaled(code) => code as i32,
                    // TODO: consider erring out on Undetermined
                    ExitStatus::Undetermined => 0,
                };
                System::current().stop_with_code(code);
            }
            System::current().stop();
        }
    }
}

