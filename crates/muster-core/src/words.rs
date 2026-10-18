//! The words a pane is called by until somebody names it (`otter`, `maple`, `cobalt`).
//!
//! A pane's id (`p29te12q4t`) is its permanent address, and nobody can remember one, so every
//! pane is also drawn one of these: a word a person can read off a row and type after `--pane`,
//! and an agent can say. One word rather than an adjective and a noun, because a name only has
//! to tell apart the panes that are open at once, and a few hundred words do that at half the
//! length. Not `claude-3`, which is wrong once the pane runs something else.
//!
//! **No two words share their first three letters**, so three keystrokes say which word you
//! mean, and a completer has nothing to choose between after them. Plain lowercase nouns of
//! four to seven letters: nothing that reads as an agent's state (`idle`, `stuck`), as one of
//! Muster's own nouns (`pane`, `tab`, `shell`), as a harness or a language, or badly when it is
//! the subject of "is stuck" or "needs you". The tests below hold the list to all of it.
//!
//! The daemon draws, because it knows which words its live panes hold and is the one that has
//! to name a pane no window asked for. It also knows what would make a word a poor choice: one
//! that appears in the pane's directory or in what its other panes are called, so that `swan`
//! is not drawn inside a project called swan, and one a pane closed with moments ago, so that
//! a name in a message still means the pane it meant. Those are preferences rather than rules,
//! because a word is never something to go without.

use flexid::{OsRandom, RandomSource};

use crate::diagnostics::monotonic_now;

/// Every word a pane can be drawn.
pub const WORDS: &[&str] = &[
    "acorn", "adder", "agate", "alder", "algae", "almond", "amber", "anchor", "anvil", "apple",
    "apricot", "arch", "aspen", "aster", "atlas", "aurora", "badger", "bagel", "bamboo", "banjo",
    "barley", "basil", "beaver", "bell", "berry", "birch", "bison", "blossom", "bobcat", "bonsai",
    "boulder", "bramble", "brook", "bubble", "buffalo", "burrow", "button", "cabin", "cactus",
    "camel", "canyon", "carrot", "castle", "cedar", "cello", "cherry", "cider", "cinder", "clover",
    "cobalt", "cocoa", "comet", "condor", "copper", "coral", "cotton", "cougar", "crane",
    "cricket", "crocus", "cumin", "cypress", "daisy", "dancer", "delta", "denim", "dingo",
    "dolphin", "domino", "dove", "dragon", "drum", "dune", "eagle", "easel", "eclipse", "egret",
    "elbow", "elder", "ember", "engine", "ermine", "falcon", "fawn", "feather", "fennel", "ferret",
    "fiddle", "field", "finch", "fjord", "flint", "flute", "forest", "fossil", "frost", "fudge",
    "galaxy", "garnet", "gazelle", "gecko", "geyser", "ginger", "glacier", "globe", "goose",
    "gopher", "grape", "gull", "guppy", "hammock", "harbor", "hazel", "heron", "hickory", "hippo",
    "holly", "honey", "hornet", "husky", "ibex", "iceberg", "igloo", "indigo", "iris", "island",
    "ivory", "jade", "jaguar", "jasmine", "jelly", "jester", "jigsaw", "journey", "juniper",
    "kayak", "kelp", "kettle", "kite", "kiwi", "knot", "koala", "ladder", "lagoon", "lake",
    "lantern", "lark", "laurel", "lava", "lemon", "lentil", "leopard", "lily", "lime", "linen",
    "lion", "lizard", "llama", "lobster", "locket", "lotus", "lynx", "magpie", "mango", "maple",
    "marble", "meadow", "melon", "mesa", "meteor", "mint", "mirror", "mitten", "mole", "monsoon",
    "moose", "moss", "moth", "mussel", "narwhal", "nebula", "nectar", "needle", "nest", "newt",
    "nickel", "nimbus", "noodle", "nutmeg", "oasis", "oatmeal", "ocean", "olive", "onyx", "opal",
    "orbit", "orchid", "oriole", "osprey", "otter", "oyster", "paddle", "panda", "papaya",
    "parrot", "peach", "pebble", "pecan", "pelican", "pepper", "petal", "pigeon", "pine", "plum",
    "pollen", "pony", "poppy", "prairie", "puffin", "pumpkin", "quartz", "quill", "quokka",
    "rabbit", "radish", "raven", "reef", "ribbon", "ripple", "river", "robin", "rocket", "rudder",
    "saffron", "sage", "salmon", "sandal", "sapling", "satin", "scarab", "seal", "sequoia",
    "shadow", "sierra", "silver", "skylark", "snail", "sonnet", "sorrel", "sparrow", "spruce",
    "squid", "stone", "summit", "sunset", "swan", "tabby", "taffy", "tango", "tapir", "teal",
    "temple", "thistle", "thunder", "tiger", "timber", "toad", "topaz", "toucan", "trout", "tulip",
    "tundra", "turtle", "umber", "unicorn", "urchin", "valley", "vanilla", "velvet", "violet",
    "vole", "voyage", "walrus", "wasp", "water", "whale", "willow", "wind", "wombat", "wren",
    "yarrow", "yodel", "yucca", "zebra", "zenith", "zephyr", "zinnia",
];

/// A word `taken` says no live pane is called, chosen with the machine's entropy, and one
/// `unwelcome` says nothing against while any such word is free.
pub fn draw(taken: impl Fn(&str) -> bool, unwelcome: impl Fn(&str) -> bool) -> String {
    let mut roll = [0u8; 8];
    if OsRandom.fill_bytes(&mut roll).is_err() {
        // Never on a platform Muster runs on; a name is not something to go without over it.
        roll = monotonic_now().to_le_bytes();
    }
    draw_from(taken, unwelcome, u64::from_le_bytes(roll))
}

/// [`draw`], chosen by `roll`, so that a test can say which.
///
/// Uniform over the free words rather than the first free one after a random start, which would
/// favour whichever word follows a run of taken ones. When every word is taken, as it is with
/// as many panes open as there are words, the words come round again with a number: `otter-2`.
pub fn draw_from(
    taken: impl Fn(&str) -> bool,
    unwelcome: impl Fn(&str) -> bool,
    roll: u64,
) -> String {
    for round in 1usize.. {
        let free: Vec<String> = WORDS
            .iter()
            .map(|word| if round == 1 { (*word).to_string() } else { format!("{word}-{round}") })
            .filter(|word| !taken(word))
            .collect();
        let welcome: Vec<String> = free.iter().filter(|word| !unwelcome(word)).cloned().collect();
        let free = if welcome.is_empty() { free } else { welcome };
        if !free.is_empty() {
            let at = usize::try_from(roll % free.len() as u64).unwrap_or_default();
            return free.into_iter().nth(at).unwrap_or_default();
        }
    }
    unreachable!("a round past every number there is")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn there_are_a_few_hundred_plain_short_words() {
        assert!(WORDS.len() >= 250, "{} words", WORDS.len());
        for word in WORDS {
            assert!((4..=7).contains(&word.len()), "{word} is not four to seven letters");
            assert!(word.bytes().all(|byte| byte.is_ascii_lowercase()), "{word} is not plain");
        }
    }

    #[test]
    fn no_two_words_share_their_first_three_letters() {
        let mut seen = BTreeMap::new();
        for word in WORDS {
            if let Some(other) = seen.insert(&word[..3], word) {
                panic!("{other} and {word} both begin {}", &word[..3]);
            }
        }
    }

    #[test]
    fn no_word_reads_as_a_state_one_of_musters_nouns_or_a_harness() {
        const NOT: &[&str] = &[
            "idle", "stuck", "done", "gone", "blocked", "working", "waiting", "human", "agent",
            "pane", "tab", "window", "shell", "muster", "daemon", "claude", "codex", "echo",
            "rust", "ruby",
        ];
        for word in NOT {
            assert!(!WORDS.contains(word), "{word} is in the list");
        }
    }

    #[test]
    fn a_drawn_word_is_one_no_live_pane_holds() {
        let held = ["otter", "maple"];
        for roll in 0..WORDS.len() as u64 {
            let word = draw_from(|word| held.contains(&word), |_| false, roll);
            assert!(!held.contains(&word.as_str()));
            assert!(WORDS.contains(&word.as_str()));
        }
    }

    #[test]
    fn once_every_word_is_held_they_come_round_again_numbered() {
        let word = draw_from(|word| !word.ends_with("-2"), |_| false, 0);
        assert_eq!(word, format!("{}-2", WORDS[0]));
        let word = draw_from(|word| !word.ends_with("-3"), |_| false, 7);
        assert_eq!(word, format!("{}-3", WORDS[7]));
    }

    #[test]
    fn the_machines_entropy_draws_a_word() {
        assert!(WORDS.contains(&draw(|_| false, |_| false).as_str()));
    }

    #[test]
    fn an_unwelcome_word_is_drawn_only_once_no_other_is_free() {
        let (otter, maple) = (WORDS[0], WORDS[1]);
        for roll in 0..WORDS.len() as u64 {
            let word = draw_from(|_| false, |word| word == otter, roll);
            assert_ne!(word, otter, "drawn though others were free");
        }
        let only_two = |word: &str| word != otter && word != maple;
        for roll in 0..4 {
            assert_eq!(draw_from(only_two, |word| word == otter, roll), maple);
        }
        assert_eq!(draw_from(|word| word != otter, |_| true, 3), otter, "never left without one");
    }
}
