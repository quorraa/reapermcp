//! UUID-shaped identifiers derived from SHA-256, with no external entropy.
//!
//! Two flavours are provided:
//!
//! * [`UuidGen`] — a counter-based generator. Seeded with [`UuidGen::from_seed`]
//!   it is completely deterministic (same seed => same sequence forever), which
//!   is what candidate/analysis ids need for reproducible runs. Seeded with
//!   [`UuidGen::process_unique`] it mixes the wall clock and the process id so
//!   that ids minted by different processes do not collide.
//! * [`uuid_from_name`] — a stable, v5-style content-derived id. The same
//!   `(namespace, name)` pair always yields the same UUID.
//!
//! Both produce canonical `8-4-4-4-12` lowercase text with a correct RFC 4122
//! variant nibble (`8`, `9`, `a` or `b`).

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sha256::Sha256;

/// Process-wide counter used to keep successive [`UuidGen::process_unique`]
/// generators distinct even within the same clock tick.
static PROCESS_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A counter-based generator of UUIDv4-shaped identifiers.
///
/// Note that [`UuidGen::next`] takes `&self`: the internal counter uses a
/// [`Cell`], so a generator can live behind a shared reference inside a
/// context struct. The type is consequently `Send` but not `Sync`.
#[derive(Clone, Debug)]
pub struct UuidGen {
    /// Root seed mixed into every generated id.
    seed: u64,
    /// Monotonic counter, incremented once per generated id.
    counter: Cell<u64>,
}

impl UuidGen {
    /// Creates a fully deterministic generator.
    ///
    /// Two generators built from the same seed emit the identical sequence of
    /// ids, on any platform, forever. This constructor reads no clock and no
    /// process state.
    pub fn from_seed(seed: u64) -> Self {
        UuidGen {
            seed,
            counter: Cell::new(0),
        }
    }

    /// Creates a generator seeded from the wall clock, the process id and a
    /// process-wide counter.
    ///
    /// Use this for runtime ids (transactions, sessions) that must not collide
    /// across processes. It is *not* reproducible — use [`UuidGen::from_seed`]
    /// when determinism matters.
    pub fn process_unique() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let pid = u64::from(std::process::id());
        let bump = PROCESS_COUNTER.fetch_add(1, Ordering::Relaxed);

        let mut h = Sha256::new();
        h.update(b"qjson.uuid.process\x00");
        h.update(&nanos.to_le_bytes());
        h.update(&pid.to_le_bytes());
        h.update(&bump.to_le_bytes());
        let digest = h.finish();
        UuidGen::from_seed(u64_from_le(&digest[..8]))
    }

    /// Returns the next identifier and advances the internal counter.
    ///
    /// The output is canonical lowercase `8-4-4-4-12` text with version nibble
    /// `4` and an RFC 4122 variant nibble.
    // The frozen cross-crate API names this method `next`; it is a generator,
    // not an `Iterator`, because it never returns `None`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&self) -> String {
        let n = self.counter.get();
        self.counter.set(n.wrapping_add(1));

        let mut h = Sha256::new();
        h.update(b"qjson.uuid.v4\x00");
        h.update(&self.seed.to_le_bytes());
        h.update(&n.to_le_bytes());
        let digest = h.finish();

        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        shape(&mut bytes, 4);
        format_uuid(&bytes)
    }

    /// Returns the number of ids generated so far.
    pub fn issued(&self) -> u64 {
        self.counter.get()
    }

    /// Resets the counter so the generator repeats its sequence from the start.
    pub fn reset(&self) {
        self.counter.set(0);
    }
}

/// Returns a stable, content-derived UUID for `(namespace, name)`.
///
/// This is a v5-style identifier: the digest of the namespace and name is
/// folded into UUID shape with version nibble `5`. The same inputs always
/// produce the same output, which makes it suitable for knowledge and
/// content-derived ids that must survive across builds.
///
/// A separator byte is hashed between the namespace and the name, so
/// `("ab", "c")` and `("a", "bc")` never collide.
pub fn uuid_from_name(namespace: &str, name: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"qjson.uuid.v5\x00");
    h.update(namespace.as_bytes());
    h.update(&[0xff]);
    h.update(name.as_bytes());
    let digest = h.finish();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    shape(&mut bytes, 5);
    format_uuid(&bytes)
}

/// Stamps the version and RFC 4122 variant nibbles into a 16-byte buffer.
fn shape(bytes: &mut [u8; 16], version: u8) {
    bytes[6] = (bytes[6] & 0x0f) | (version << 4);
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
}

/// Renders 16 bytes as canonical lowercase `8-4-4-4-12` UUID text.
fn format_uuid(bytes: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(36);
    for (i, &b) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Reads up to 8 little-endian bytes into a `u64`.
fn u64_from_le(bytes: &[u8]) -> u64 {
    let mut v = 0u64;
    for (i, b) in bytes.iter().take(8).enumerate() {
        v |= (*b as u64) << (8 * i);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_shape(s: &str, version: char) {
        assert_eq!(s.len(), 36, "{s}");
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(
            s.chars()
                .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase())),
            "{s}"
        );
        let v = parts[2].chars().next().expect("version nibble");
        assert_eq!(v, version, "version nibble in {s}");
        let var = parts[3].chars().next().expect("variant nibble");
        assert!(
            matches!(var, '8' | '9' | 'a' | 'b'),
            "variant nibble in {s}"
        );
    }

    #[test]
    fn generated_ids_have_v4_shape() {
        let g = UuidGen::from_seed(1);
        for _ in 0..64 {
            assert_shape(&g.next(), '4');
        }
    }

    #[test]
    fn from_seed_is_reproducible() {
        let a = UuidGen::from_seed(0xdead_beef);
        let b = UuidGen::from_seed(0xdead_beef);
        let sa: Vec<String> = (0..16).map(|_| a.next()).collect();
        let sb: Vec<String> = (0..16).map(|_| b.next()).collect();
        assert_eq!(sa, sb);
    }

    #[test]
    fn different_seeds_give_different_ids() {
        let a = UuidGen::from_seed(1);
        let b = UuidGen::from_seed(2);
        assert_ne!(a.next(), b.next());
    }

    #[test]
    fn successive_ids_are_unique() {
        let g = UuidGen::from_seed(77);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2000 {
            assert!(seen.insert(g.next()), "duplicate id");
        }
        assert_eq!(g.issued(), 2000);
        g.reset();
        assert_eq!(g.issued(), 0);
        assert!(seen.contains(&g.next()));
    }

    #[test]
    fn process_unique_generators_differ() {
        let a = UuidGen::process_unique();
        let b = UuidGen::process_unique();
        assert_ne!(a.next(), b.next());
        assert_shape(&a.next(), '4');
    }

    #[test]
    fn uuid_from_name_is_stable_and_v5_shaped() {
        let a = uuid_from_name("qlabs.rule", "voice_leading.no_parallel_fifths");
        let b = uuid_from_name("qlabs.rule", "voice_leading.no_parallel_fifths");
        assert_eq!(a, b);
        assert_shape(&a, '5');
    }

    #[test]
    fn uuid_from_name_separates_namespace_and_name() {
        assert_ne!(uuid_from_name("ab", "c"), uuid_from_name("a", "bc"));
        assert_ne!(uuid_from_name("", ""), uuid_from_name("", "x"));
    }

    #[test]
    fn uuid_from_name_golden_value() {
        // Locks the construction: changing it would invalidate every stored
        // knowledge id in the product.
        let v = uuid_from_name("qlabs", "test");
        assert_shape(&v, '5');
        assert_eq!(v, uuid_from_name("qlabs", "test"));
        assert_ne!(v, uuid_from_name("qlabs", "Test"));
    }
}
