//! Deterministic fuzzing of plan wire v3: truncated, byte-mutated and token-mutated versions
//! of the JVM's index plan, and pathological nesting, are rejected with an error or decoded
//! into a plan that passed validation; the decoder never panics.
use adb_plan_wire::{decode_json, encode_json, PlanWireError};

/// The JVM's index plan fixture (index scan, filter, row fetch, project).
const FIXTURE: &str = include_str!("fixtures/jvm-index-plan.json");

/// Deterministic pseudo-random words (xorshift64).
struct Words(u64);

impl Words {
    /// Next word.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `0..bound`.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// Decodes `bytes`; a decoded plan must re-validate and re-encode (it passed validation).
fn check(bytes: &[u8]) -> Result<(), PlanWireError> {
    let (plan, _) = decode_json(bytes)?;
    plan.validate().expect("decoded plans are valid");
    encode_json(&plan).expect("decoded plans re-encode");
    Ok(())
}

/// Every proper prefix of the plan is malformed.
#[test]
fn truncations_are_malformed() {
    check(FIXTURE.trim().as_bytes()).unwrap();
    let bytes = FIXTURE.trim().as_bytes();
    for cut in 0..bytes.len() {
        assert!(check(&bytes[..cut]).is_err(), "prefix of {cut} bytes");
    }
}

/// Random byte mutations never panic the decoder.
#[test]
fn byte_mutations_never_panic() {
    let original = FIXTURE.trim().as_bytes().to_vec();
    let mut words = Words(0x5EED);
    let specials = b"{}[]\":,-0123456789eE.\\ \x00\xff";
    let mut accepted = 0;
    for _ in 0..20_000 {
        let mut bytes = original.clone();
        for _ in 0..words.below(4) + 1 {
            let at = words.below(bytes.len());
            match words.below(3) {
                0 => bytes[at] = specials[words.below(specials.len())],
                1 => {
                    bytes.remove(at);
                }
                _ => bytes.insert(at, specials[words.below(specials.len())]),
            }
        }
        if check(&bytes).is_ok() {
            accepted += 1;
        }
    }
    assert!(accepted < 20_000, "most mutations are rejected");
}

/// Mutating the values of known tokens (numbers out of range, wrong types, unknown enum
/// tags, NULL bounds, too many key columns) is rejected, never accepted silently.
#[test]
fn token_mutations_are_rejected() {
    let fixture = FIXTURE.trim();
    let replacements = [
        ("\"index_id\":4", "\"index_id\":-4"),
        ("\"index_id\":4", "\"index_id\":18446744073709551616"),
        ("\"index_id\":4", "\"index_id\":\"4\""),
        ("\"direction\":\"backward\"", "\"direction\":\"sideways\""),
        ("\"limit\":10", "\"limit\":2000000"),
        ("\"limit\":10", "\"limit\":-1"),
        ("\"inclusive\":false", "\"inclusive\":0"),
        ("{\"Int64\":-5}", "{\"Int64\":9223372036854775808}"),
        ("{\"Int64\":-5}", "\"Null\""),
        ("{\"Int64\":-5}", "{\"Float64\":\"NaN\"}"),
        ("{\"String\":\"Kraków\"}", "{\"Int32\":1}"),
        ("\"slot\":2}", "\"slot\":5000}"),
        ("\"slot\":0}", "\"slot\":1}"),
        ("\"op\":\"row_fetch\"", "\"op\":\"row_fetch_v4\""),
        ("\"wire_version\":3", "\"wire_version\":4"),
        ("\"wire_version\":3", "\"wire_version\":2"),
    ];
    for (from, to) in replacements {
        assert!(fixture.contains(from), "{from}");
        let mutated = fixture.replacen(from, to, 1);
        assert!(check(mutated.as_bytes()).is_err(), "{to} must be rejected");
    }
    let wide = (0..17)
        .map(|n| format!("{{\"Int64\":{n}}}"))
        .collect::<Vec<_>>()
        .join(",");
    let mutated = fixture.replacen("[{\"String\":\"Kraków\"}]", &format!("[{wide}]"), 1);
    assert!(check(mutated.as_bytes()).is_err(), "17 prefix values");
}

/// Deep nesting of row fetches and filters is refused by the depth limit (or the JSON parser's
/// recursion limit), not by a stack overflow.
#[test]
fn deep_nesting_is_refused() {
    let scan = r#"{"op":"index_scan","index_id":1,"bounds":{"prefix":[]},"columns":[]}"#;
    for depth in [130usize, 100_000] {
        let plan = format!(
            "{}{scan}{}",
            r#"{"op":"limit","input":"#.repeat(depth),
            r#","limit":1}"#.repeat(depth)
        );
        let wire = format!(r#"{{"wire_version":3,"plan":{plan}}}"#);
        assert!(check(wire.as_bytes()).is_err(), "depth {depth}");
    }
}
