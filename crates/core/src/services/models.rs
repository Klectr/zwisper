//! Model downloads and tier switching.

use super::SharedCtx;
use crate::bus::Event;
use crate::cancel::CancelToken;
use crate::domain::*;
use crate::error::Error;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct ModelService {
    downloads: Arc<Mutex<HashMap<Pack, CancelToken>>>,
}

pub fn current_tier(ctx: &SharedCtx) -> Tier {
    ctx.ports
        .settings
        .load()
        .tier
        .unwrap_or_else(|| Tier::for_memory_bytes(ctx.ports.system_memory_bytes))
}

impl ModelService {
    pub fn download(&self, ctx: &SharedCtx, pack: Pack) {
        let mut d = self.downloads.lock();
        if d.contains_key(&pack) {
            return;
        }
        let cancel = CancelToken::new();
        d.insert(pack, cancel.clone());
        let ctx = ctx.clone();
        let downloads = self.downloads.clone();
        std::thread::Builder::new()
            .name(format!("download-{pack:?}"))
            .spawn(move || {
                let tier = current_tier(&ctx);
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut progress = |done: u64, total: u64| {
                    if last.elapsed() >= Duration::from_millis(200) || done == total {
                        last = Instant::now();
                        ctx.events.publish(Event::Pack { pack, status: PackStatus::Downloading { done, total } });
                    }
                };
                let res = ctx.ports.models.download(pack, tier, &mut progress, &cancel);
                downloads.lock().remove(&pack);
                match res {
                    Ok(()) => {
                        // Engines load lazily, so nothing to rebuild; just report.
                        ctx.events.publish(Event::Pack { pack, status: ctx.ports.models.status(pack, tier) });
                    }
                    Err(Error::Cancelled) => {
                        ctx.events.publish(Event::Pack { pack, status: ctx.ports.models.status(pack, tier) });
                    }
                    Err(e) => {
                        ctx.events.publish(Event::Pack { pack, status: PackStatus::Failed(e.to_string()) });
                        ctx.events.publish(Event::Error(format!("Download failed: {e}")));
                    }
                }
            })
            .ok();
    }

    pub fn cancel(&self, pack: Pack) {
        if let Some(c) = self.downloads.lock().get(&pack) {
            c.cancel();
        }
    }

    pub fn remove(&self, ctx: &SharedCtx, pack: Pack) {
        let tier = current_tier(ctx);
        // Make sure nothing holds the files open.
        match pack {
            Pack::Speech => ctx.engines.transcriber.current().lock().unload(),
            Pack::Speakers => ctx.engines.diarizer.current().lock().unload(),
            Pack::Notes => ctx.engines.notes.current().lock().unload(),
        }
        if let Err(e) = ctx.ports.models.remove(pack, tier) {
            ctx.events.publish(Event::Error(format!("Could not remove model: {e}")));
        }
        ctx.events.publish(Event::Pack { pack, status: ctx.ports.models.status(pack, tier) });
    }

    /// Switch Standard ↔ Lite: hot-swap engines; the next job uses the new tier.
    pub fn set_tier(&self, ctx: &SharedCtx, tier: Tier) {
        let mut s = ctx.ports.settings.load();
        s.tier = Some(tier);
        if let Err(e) = ctx.ports.settings.save(&s) {
            ctx.events.publish(Event::Error(format!("Could not save settings: {e}")));
            return;
        }
        if let Err(e) = ctx.engines.retier(tier) {
            ctx.events.publish(Event::Error(format!("Could not switch models: {e}")));
            return;
        }
        ctx.events.publish(Event::TierChanged(tier));
        ctx.events.publish(Event::SettingsChanged(s));
        for pack in Pack::ALL {
            ctx.events.publish(Event::Pack { pack, status: ctx.ports.models.status(pack, tier) });
        }
    }
}
