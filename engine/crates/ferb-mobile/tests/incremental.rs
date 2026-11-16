//! The fold reached entry by entry equals the fold of the whole list, for every scenario: entries one at a time in
//! a shuffled order, in pages split at random points (newest page first, as a phone pages back), duplicated, with
//! older revisions arriving after newer ones and tombstones arriving last. Seeded, so every run is the same.

mod common;

use common::{conversations, scenarios, Rng};
use ferb_mobile::entries::{Applied, EntrySet};
use ferb_mobile::entry::ChatEntry;
use ferb_mobile::fold;
use serde_json::json;

const TZ: i32 = 0;

/// An older revision of an entry: what a phone may have fetched before the change (different words, one rev and
/// one seq lower).
fn older(e: &ChatEntry) -> ChatEntry {
    let mut o = e.clone();
    o.rev = (e.rev - 1).max(0);
    o.seq = e.seq - 1;
    o.deleted = false;
    o.body
        .insert("text".into(), json!("an older revision's words"));
    o
}

// decision: ENG-10
// decision: PHN-408
// decision: ENG-8
// decision: PHN-473
#[test]
fn entries_in_any_order_fold_to_the_whole_lists_transcript() {
    let mut rng = Rng::new(0x5EED_0007);
    let mut runs = 0;
    for (file, name, entries) in scenarios() {
        for (c, mine) in conversations(&entries) {
            let whole = fold::transcript(&mine, TZ);
            for round in 0..6 {
                let mut feed: Vec<ChatEntry> = mine.clone();
                // older revisions of revised entries, and duplicates
                for e in &mine {
                    if e.rev > 1 && rng.below(2) == 0 {
                        feed.push(older(e));
                    }
                    if rng.below(4) == 0 {
                        feed.push(e.clone());
                    }
                }
                let mut set = EntrySet::new();
                match round % 3 {
                    // one at a time, shuffled
                    0 => {
                        rng.shuffle(&mut feed);
                        for e in feed {
                            set.apply(e);
                        }
                    }
                    // pages by pos, newest page first, split at random points, each page shuffled
                    1 => {
                        feed.sort_by_key(|e| e.pos);
                        let mut cuts: Vec<usize> = (0..rng.below(5))
                            .map(|_| rng.below(feed.len() + 1))
                            .collect();
                        cuts.push(0);
                        cuts.push(feed.len());
                        cuts.sort();
                        cuts.dedup();
                        for w in cuts.windows(2).rev() {
                            let mut page = feed[w[0]..w[1]].to_vec();
                            rng.shuffle(&mut page);
                            for e in page {
                                set.apply(e);
                            }
                        }
                    }
                    // tombstones and revisions last (changes since a cursor arrive after the pages)
                    _ => {
                        feed.sort_by_key(|e| (e.deleted || e.rev > 1, e.seq));
                        for e in feed {
                            set.apply(e);
                        }
                    }
                }
                assert_eq!(
                    set.transcript(TZ),
                    whole,
                    "{file} {name} {c}: round {round}"
                );
                runs += 1;
            }
        }
    }
    assert!(runs >= 146 * 6);
}

/// A tombstone fetched after the entry it empties takes the entry out, as the whole list with it does.
// decision: PHN-408
// decision: ENG-8
// decision: PHN-473
#[test]
fn a_late_tombstone_removes_what_it_deleted() {
    let mut rng = Rng::new(42);
    for (file, name, entries) in scenarios() {
        for (c, mine) in conversations(&entries) {
            let live: Vec<&ChatEntry> = mine.iter().filter(|e| !e.deleted).collect();
            if live.is_empty() {
                continue;
            }
            let victim = live[rng.below(live.len())];
            let mut tomb = victim.clone();
            tomb.rev += 1;
            tomb.seq = mine.iter().map(|e| e.seq).max().unwrap_or(0) + 1;
            tomb.deleted = true;
            tomb.body = Default::default();
            let mut with: Vec<ChatEntry> = mine.clone();
            with.push(tomb.clone());
            let expected = fold::transcript(&with, TZ);
            let mut set = EntrySet::from_entries(mine.clone());
            assert_eq!(set.apply(tomb), Applied::Kept);
            assert_eq!(set.transcript(TZ), expected, "{file} {name} {c}");
            assert!(
                !set.transcript(TZ)
                    .items
                    .iter()
                    .any(|i| i.entry() == victim.id),
                "{file} {name}: the deleted entry leaves no item"
            );
        }
    }
}

/// An entry whose words a reply cut is never folded in place of the whole one.
// decision: ENG-14
// decision: PHN-473
#[test]
fn a_cut_entry_never_replaces_a_whole_one() {
    let (_, _, entries) = scenarios().into_iter().next().unwrap();
    let mut set = EntrySet::from_entries(entries.clone());
    let before = set.transcript(TZ);
    let mut cut = entries[0].clone();
    cut.rev += 5;
    cut.seq += 100;
    cut.cut = true;
    cut.body.insert("text".into(), json!("hi th"));
    assert_eq!(set.apply(cut), Applied::Cut);
    assert_eq!(set.transcript(TZ), before);
}
