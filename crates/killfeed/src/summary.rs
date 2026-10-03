//! What a clip's kills add up to for the recording player.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{KILL_MODIFIERS, Kill, Owner};

/// Kills further apart than this are taken to be in different rounds: a round's own kills
/// rarely sit more than a few seconds apart, and the next round starts after the round-end
/// screen and freeze time.
const ROUND_GAP_S: f64 = 40.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    /// Kills in the clip, anyone's.
    pub kills: usize,
    /// The recording player's kills (red-outlined rows).
    pub my_kills: usize,
    /// The recording player's deaths (red-filled rows).
    pub my_deaths: usize,
    /// Most of the player's kills in one round: `2k`, `3k`, `4k` or `ace`.
    pub multi_kill: Option<String>,
    /// The player's kills per weapon (CS2 names: `ak47`, `awp`, ...).
    pub weapons: BTreeMap<String, usize>,
    /// The player's kills per modifier (`headshot`, `wallbang`, ...).
    pub modifiers: BTreeMap<String, usize>,
}

pub fn summarise(kills: &[Kill]) -> Summary {
    let mine: Vec<&Kill> = kills.iter().filter(|k| k.owner == Owner::MyKill).collect();
    let mut summary = Summary {
        kills: kills.len(),
        my_kills: mine.len(),
        my_deaths: kills.iter().filter(|k| k.owner == Owner::MyDeath).count(),
        ..Summary::default()
    };
    // Kills come sorted by time; split them into rounds at long gaps.
    let (mut best, mut run, mut last) = (0, 0, f64::NEG_INFINITY);
    for kill in &mine {
        run = if kill.t - last > ROUND_GAP_S {
            1
        } else {
            run + 1
        };
        best = best.max(run);
        last = kill.t;
        if let Some(weapon) = &kill.weapon {
            *summary.weapons.entry(weapon.clone()).or_default() += 1;
        }
        for modifier in kill
            .modifiers
            .iter()
            .filter(|m| KILL_MODIFIERS.contains(&m.as_str()))
        {
            *summary.modifiers.entry(modifier.clone()).or_default() += 1;
        }
    }
    summary.multi_kill = match best {
        0 | 1 => None,
        2..=4 => Some(format!("{best}k")),
        _ => Some("ace".into()),
    };
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Row;

    fn kill(t: f64, owner: Owner, weapon: &str, modifiers: &[&str]) -> Kill {
        Kill {
            t,
            last_seen: t + 6.0,
            sightings: 10,
            owner,
            weapon: Some(weapon.into()),
            modifiers: modifiers.iter().map(|m| (*m).to_owned()).collect(),
            example: (
                t,
                Row {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 1.0,
                    y1: 1.0,
                    score: 1.0,
                },
            ),
        }
    }

    #[test]
    fn counts_the_players_kills_and_multi_kill() {
        // Moshiloko's clip: four AK kills in one round, one through smoke, one headshot,
        // one wallbang, plus other players' kills.
        let kills = [
            kill(10.0, Owner::Other, "fiveseven", &["headshot"]),
            kill(18.0, Owner::MyKill, "ak47", &[]),
            kill(19.0, Owner::MyKill, "ak47", &["through_smoke"]),
            kill(24.0, Owner::Other, "mp7", &[]),
            kill(29.0, Owner::MyKill, "ak47", &["headshot"]),
            kill(34.0, Owner::MyKill, "ak47", &["wallbang", "domination"]),
        ];
        let s = summarise(&kills);
        assert_eq!((s.kills, s.my_kills, s.my_deaths), (6, 4, 0));
        assert_eq!(s.multi_kill.as_deref(), Some("4k"));
        assert_eq!(s.weapons["ak47"], 4);
        assert_eq!(s.modifiers.len(), 3, "domination is not a kill modifier");
    }

    #[test]
    fn rounds_split_at_long_gaps() {
        let kills = [
            kill(5.0, Owner::MyKill, "awp", &["noscope"]),
            kill(9.0, Owner::MyKill, "awp", &[]),
            kill(95.0, Owner::MyKill, "ak47", &[]),
            kill(150.0, Owner::MyDeath, "ak47", &["headshot"]),
        ];
        let s = summarise(&kills);
        assert_eq!(s.multi_kill.as_deref(), Some("2k"));
        assert_eq!(s.my_deaths, 1);
    }

    #[test]
    fn five_in_a_round_is_an_ace() {
        let kills: Vec<Kill> = (0..5)
            .map(|i| kill(10.0 + 3.0 * f64::from(i), Owner::MyKill, "deagle", &[]))
            .collect();
        let s = summarise(&kills);
        assert_eq!(s.multi_kill.as_deref(), Some("ace"));
        assert_eq!(s.weapons["deagle"], 5);
        // One kill is no multi-kill, and no kills is nothing at all.
        assert_eq!(summarise(&kills[..1]).multi_kill, None);
        assert_eq!(summarise(&[]), Summary::default());
    }
}
