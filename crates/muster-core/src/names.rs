//! Muster's own names for panes and tabs.
//!
//! A pane or tab is known to its daemon by the name Muster minted and passed in the request
//! that made it, so the name is the address everywhere: in `muster window`, on a CLI line, in
//! a saved arrangement, and to the daemon (MIP-3, section 2). Nothing here binds a name to
//! anything else, because there is nothing else to bind it to.
//!
//! **A pane is named so that it can be told which pane it is.** The name goes into the request
//! that creates the pane, and the pane's process is born knowing it as `MUSTER_PANE`. That is
//! what lets an agent in a pane say "below me" (`architecture.md`, one action path).
//!
//! **A tab is named so that it can be addressed**, by a script, a CLI or an agent. A tab that
//! spans two machines is one name held on both daemons, which is the whole of the grouping.
//!
//! A name is a letter and nine characters - `p1w3r07bsd`, `t1w3r07bsd` - of which the first
//! five say when the thing was made, to the nearest ten seconds, and the last four keep things
//! made within one of those apart. So names sort into the order they were made, and two Musters
//! that never speak to each other cannot mint one name unless they mint within ten seconds of
//! each other. The letter says which noun, so a name never reads as the position number a
//! sidebar draws beside it, and a tab name can never be mistaken for a pane's.
//!
//! **Names are globally unique, which is a property and not an accident**: a minted name is an
//! answer on its own, which is what lets a CLI reach a pane or a tab on the devenv without
//! knowing which daemon holds it. **Never reused**, so a name that outlives what it named
//! resolves to nothing rather than to somebody else's work.

use std::collections::BTreeSet;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flexid::{Alphabet, Generator, OsRandom, RandomError, RandomSource};

use crate::diagnostics::monotonic_now;
use crate::mirror::backend::{PaneId, TabId};

/// Where the next name comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mint {
    /// The machine's clock and the machine's entropy, which is what a running Muster uses.
    Drawn,

    /// The same spelling at an instant the caller fixes, with entropy from a seed.
    ///
    /// flexid takes both impure inputs as arguments, so the same instant and the same seed
    /// always produce the same name. That is what lets a conformance case pin the name a draw
    /// produces rather than only the shape one has, which matters because a name that changed
    /// shape between versions would strand every pane already carrying one in its
    /// environment.
    Replayed { at: SystemTime, seed: u64 },
}

impl Mint {
    fn draw(&mut self, prefix: char) -> String {
        match self {
            Mint::Drawn => spell(SystemTime::now(), &mut OsRandom, prefix),
            Mint::Replayed { at, seed } => {
                // Zero is xorshift's fixed point, so a case that gave it would draw one name
                // over and over and exhaust the collision retries instead of saying why.
                if *seed == 0 {
                    *seed = 1;
                }
                spell(*at, &mut Seeded(seed), prefix)
            }
        }
    }
}

/// Draws names, never the same one twice.
#[derive(Debug)]
pub struct Minter {
    mint: Mint,
    /// Every name this has handed out, so that two drawn within one tick cannot be the same.
    drawn: BTreeSet<String>,
}

impl Minter {
    pub fn new(mint: Mint) -> Minter {
        Minter { mint, drawn: BTreeSet::new() }
    }

    pub fn pane(&mut self) -> PaneId {
        PaneId::new(self.draw('p'))
    }

    pub fn tab(&mut self) -> TabId {
        TabId::new(self.draw('t'))
    }

    /// A name this has never handed out. Re-drawn on a collision rather than accepted, which is
    /// what "never reused" costs.
    fn draw(&mut self, prefix: char) -> String {
        for _ in 0..64 {
            let drawn = self.mint.draw(prefix);
            if self.drawn.insert(drawn.clone()) {
                return drawn;
            }
        }
        // Sixty-four collisions in a row is not a state to recover from, and handing out a name
        // already in use would give one pane's keystrokes to another.
        panic!("could not draw a name that was not already handed out after 64 tries");
    }
}

/// Crockford's base32: no `i`, `l`, `o` or `u`, so nothing reads as something else.
///
/// Lowercase, rather than flexid's uppercase [`Alphabet::CROCKFORD_BASE32`], because a name is
/// something somebody types after `--pane`. Still in ascending byte order, which is what
/// flexid needs for names to sort.
const ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

/// How a name is spelled, decided once.
///
/// The same generator for a running Muster and for a replayed case, so that a name pinned in
/// the corpus is a name Muster actually mints.
///
/// **A 2026 epoch and ten-second ticks** are chosen together, and the pairing is the whole
/// trick. flexid does not pad the tick count, so a name grows a character each time the count
/// crosses a power of 32 - and across that boundary the shorter old names sort *after* the
/// longer new ones. Ten-second ticks from 2026 hold the count at five characters from May 2026
/// until **August 2036**, which is long enough to say plainly what a name is. One-second ticks
/// would have crossed in January 2027.
///
/// **Four random characters** is 1,048,576 names per tick, and they only have to cover two
/// Musters minting in the same ten seconds without talking to each other: within one, the
/// registry below re-draws on a collision.
// Seconds rather than the days clippy prefers: 1767225600 is a Unix timestamp, which a reader
// can recognize and look up. 20454 days is a number nobody can place.
#[allow(clippy::duration_suboptimal_units)]
fn spelling() -> &'static Generator {
    static SPELLING: LazyLock<Generator> = LazyLock::new(|| {
        Generator::builder()
            // 2026-01-01, a few months before Muster's first pane. Everything before it is
            // time no name has to spend characters encoding.
            .epoch(UNIX_EPOCH + Duration::from_secs(1_767_225_600))
            .tick_size(Duration::from_secs(10))
            .alphabet(Alphabet::new(ALPHABET).expect("base32 is 32 distinct ASCII characters"))
            .random_chars(4)
            .build()
            .expect("a ten-second tick is not zero")
    });
    &SPELLING
}

/// The noun's letter, and then what flexid says for this instant.
///
/// The letter says which noun the name names, so that a name never reads as the position
/// number the sidebar shows beside it, and so that a tab name and a pane name can never be
/// confused for one another by whoever is reading a script.
///
/// The instant is clamped to the epoch rather than passed through, because a machine whose
/// clock is set before 2026 would otherwise be refused a name outright - and a pane with no
/// name is a split missing from the window for no stated reason. Those names all sit in tick
/// zero: still distinct, they just stop saying when.
fn spell(at: SystemTime, entropy: &mut impl RandomSource, prefix: char) -> String {
    let generator = spelling();
    let at = at.max(generator.epoch());
    let drawn = generator
        .generate_at(at, entropy)
        .or_else(|_| generator.generate_at(at, &mut FromTheClock))
        // Unreachable: the fallback reads no OS entropy, the clamp rules out an instant
        // before the epoch, and one-second ticks cannot overflow a u64 this side of the heat
        // death. An empty name would be caught by the collision check either way.
        .unwrap_or_default();
    format!("{prefix}{drawn}")
}

/// A last resort when the machine will not hand over entropy.
///
/// `getrandom` does not fail on any platform Muster runs on. It is caught anyway because
/// naming is not something this can decline to do: every path that sees a pane arrives here,
/// so a refusal would cost a split rather than a character. The monotonic clock reads in tens
/// of nanoseconds, so two reads differ in their low byte, and the registry's collision check
/// covers the rest.
struct FromTheClock;

impl RandomSource for FromTheClock {
    fn fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RandomError> {
        for byte in dest.iter_mut() {
            *byte = u8::try_from(monotonic_now() & 0xff).unwrap_or_default();
        }
        Ok(())
    }
}

/// Entropy a case can reproduce, standing in for the machine's.
///
/// xorshift64*, which is four lines and does not have to be strong: what a replayed draw needs
/// is that one seed always gives one sequence, not that a name is unguessable. The socket a
/// name is spoken over is already the user's own.
struct Seeded<'a>(&'a mut u64);

impl RandomSource for Seeded<'_> {
    fn fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RandomError> {
        for byte in dest.iter_mut() {
            *self.0 ^= *self.0 >> 12;
            *self.0 ^= *self.0 << 25;
            *self.0 ^= *self.0 >> 27;
            // The top byte, because xorshift64*'s low bits are its weakest.
            *byte =
                u8::try_from(self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56).unwrap_or_default();
        }
        Ok(())
    }
}
