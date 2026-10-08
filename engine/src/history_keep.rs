// SPDX-License-Identifier: GPL-3.0-or-later

//! A switch read once from the environment that carries the history table from one `go` to the
//! next. It is an instrument on a branch that never merges, and unset it changes nothing.

use std::sync::{Mutex, OnceLock, PoisonError};

use crate::history::History;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifetime {
    Cleared,
    Kept,
    /// The kept path end to end with the table cleared anyway, so it must read as `Cleared` does.
    Planted,
}

fn lifetime() -> Lifetime {
    static LIFETIME: OnceLock<Lifetime> = OnceLock::new();
    *LIFETIME.get_or_init(
        || match std::env::var("CADENCE_HISTORY_LIFETIME").as_deref() {
            Ok("kept") => Lifetime::Kept,
            Ok("planted") => Lifetime::Planted,
            _ => Lifetime::Cleared,
        },
    )
}

static SAVED: Mutex<Option<History>> = Mutex::new(None);

/// At the start of a search, after its own clear.
pub fn restore(history: &mut History) {
    let lifetime = lifetime();
    if lifetime == Lifetime::Cleared {
        return;
    }
    let saved = SAVED.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(saved) = saved {
        *history = saved;
        if lifetime == Lifetime::Planted {
            history.clear();
        }
    }
}

/// At the end of a search, so the next `go` can take what this one learned.
pub fn save(history: &mut History) {
    if lifetime() == Lifetime::Cleared {
        return;
    }
    let taken = std::mem::take(history);
    *SAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(taken);
}

/// `ucinewgame`: nothing carries into a new game.
pub fn new_game() {
    *SAVED.lock().unwrap_or_else(PoisonError::into_inner) = None;
}
