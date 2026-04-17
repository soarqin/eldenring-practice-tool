use pinyin::ToPinyinMulti;

/// Single character's pinyin data (all readings for polyphone support)
#[derive(Debug, Clone)]
pub(crate) struct CharPinyin {
    /// All possible full-pinyin readings, e.g. ["chang", "zhang"] for 长
    pub(crate) readings: Vec<&'static str>,
    /// All possible first letters, e.g. ["c", "z"] for 长
    pub(crate) first_letters: Vec<&'static str>,
    /// The character (lowercased for non-CJK)
    pub(crate) ch: char,
    /// Whether this is a CJK character
    pub(crate) is_cjk: bool,
}

/// Complete pinyin segment sequence for one searchable item name
#[derive(Debug, Clone)]
pub(crate) struct Segment(pub(crate) Vec<CharPinyin>);

impl Segment {
    /// Build a pinyin segment sequence from an item name.
    /// - CJK chars: readings and first_letters from the `pinyin` crate static
    ///   tables (zero allocation)
    /// - Non-CJK chars: readings=[], first_letters=[], ch=lowercased char,
    ///   is_cjk=false
    pub(crate) fn from_name(name: &str) -> Self {
        let chars = name
            .chars()
            .map(|ch| {
                if let Some(pinyins) = ch.to_pinyin_multi() {
                    let (readings, first_letters): (Vec<&'static str>, Vec<&'static str>) =
                        pinyins.into_iter().map(|py| (py.plain(), py.first_letter())).unzip();
                    CharPinyin { readings, first_letters, ch, is_cjk: true }
                } else {
                    // Non-CJK: lowercase the char, no pinyin readings
                    let lower = ch.to_lowercase().next().unwrap_or(ch);
                    CharPinyin { readings: vec![], first_letters: vec![], ch: lower, is_cjk: false }
                }
            })
            .collect();
        Segment(chars)
    }
}

/// Core first-letter matching against a character slice.
fn match_first_letter_at(query_chars: &[char], chars: &[CharPinyin]) -> bool {
    if query_chars.len() > chars.len() {
        return false;
    }

    for (i, &qc) in query_chars.iter().enumerate() {
        let cp = &chars[i];
        if cp.is_cjk {
            if !cp.first_letters.iter().any(|fl| fl.starts_with(qc)) {
                return false;
            }
        } else if cp.ch != qc {
            return false;
        }
    }

    true
}

/// First-letter mode from the beginning of a segment.
fn match_first_letter(query: &str, segment: &Segment) -> bool {
    if query.is_empty() {
        return true;
    }
    let query_chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    match_first_letter_at(&query_chars, &segment.0)
}

/// Core full-pinyin matching against a character slice.
fn match_full_pinyin_at(query_lower: &str, chars: &[CharPinyin]) -> bool {
    if query_lower.len() > 128 {
        return false;
    }
    fp_match(query_lower, chars)
}

/// Full-pinyin mode from the beginning of a segment.
fn match_full_pinyin(query: &str, segment: &Segment) -> bool {
    if query.is_empty() {
        return true;
    }
    let query_lower = query.to_lowercase();
    match_full_pinyin_at(&query_lower, &segment.0)
}

/// Try to match `remaining` against `segs` starting from beginning.
/// Returns true if remaining can be fully consumed by the segment sequence.
fn fp_match(remaining: &str, segs: &[CharPinyin]) -> bool {
    if remaining.is_empty() {
        return true;
    }
    if segs.is_empty() {
        return false;
    }

    let cp = &segs[0];
    let rest_segs = &segs[1..];
    let is_last = rest_segs.is_empty();

    if cp.is_cjk {
        for &reading in &cp.readings {
            if is_last {
                if reading.starts_with(remaining) || remaining == reading {
                    return true;
                }
            } else if let Some(tail) = remaining.strip_prefix(reading) {
                if fp_match(tail, rest_segs) {
                    return true;
                }
            }
        }

        false
    } else {
        let ch_lower = cp.ch;

        if is_last {
            let mut buf = [0u8; 4];
            let ch_str = ch_lower.encode_utf8(&mut buf);
            ch_str.starts_with(remaining) || remaining == ch_str
        } else {
            let ch_len = ch_lower.len_utf8();
            if remaining.len() < ch_len {
                return false;
            }

            let (head, tail) = remaining.split_at(ch_len);
            let mut buf = [0u8; 4];
            let ch_str = ch_lower.encode_utf8(&mut buf);
            if head == ch_str {
                fp_match(tail, rest_segs)
            } else {
                false
            }
        }
    }
}

/// Pinyin match with partial matching: tries matching the query starting
/// from every position in the segment. Returns true if any position matches.
pub(crate) fn pinyin_match(query: &str, segment: &Segment) -> bool {
    if query.is_empty() {
        return true;
    }

    let query_chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let query_lower = query.to_lowercase();
    let segs = &segment.0;

    for start in 0..segs.len() {
        let slice = &segs[start..];
        if match_first_letter_at(&query_chars, slice) || match_full_pinyin_at(&query_lower, slice) {
            return true;
        }
    }
    false
}

/// Pinyin match from start only (首字匹配). Used for warp location scoring.
pub(crate) fn pinyin_match_start(query: &str, segment: &Segment) -> bool {
    if query.is_empty() {
        return true;
    }

    match_first_letter(query, segment) || match_full_pinyin(query, segment)
}

#[cfg(test)]
mod tests {
    mod first_letter {
        use super::super::{match_first_letter, Segment};

        fn seg(name: &str) -> Segment {
            Segment::from_name(name)
        }

        #[test]
        fn test_fl_exact_match() {
            assert!(match_first_letter("bs", &seg("匕首")));
        }

        #[test]
        fn test_fl_prefix_match() {
            assert!(match_first_letter("b", &seg("匕首")));
        }

        #[test]
        fn test_fl_too_long() {
            assert!(!match_first_letter("bshd", &seg("匕首")));
        }

        #[test]
        fn test_fl_mismatch() {
            assert!(!match_first_letter("bz", &seg("匕首")));
        }

        #[test]
        fn test_fl_heteronym_c() {
            assert!(match_first_letter("c", &seg("长")));
        }

        #[test]
        fn test_fl_heteronym_z() {
            assert!(match_first_letter("z", &seg("长")));
        }

        #[test]
        fn test_fl_case_insensitive() {
            assert!(match_first_letter("BS", &seg("匕首")));
            assert!(match_first_letter("Bs", &seg("匕首")));
        }

        #[test]
        fn test_fl_empty_query() {
            assert!(match_first_letter("", &seg("匕首")));
        }

        #[test]
        fn test_fl_empty_segment() {
            assert!(!match_first_letter("b", &seg("")));
        }
    }

    mod full_pinyin {
        use super::super::{match_full_pinyin, Segment};

        fn seg(name: &str) -> Segment {
            Segment::from_name(name)
        }

        #[test]
        fn test_fp_exact() {
            assert!(match_full_pinyin("bishou", &seg("匕首")));
        }

        #[test]
        fn test_fp_partial_last() {
            assert!(match_full_pinyin("bish", &seg("匕首")));
        }

        #[test]
        fn test_fp_first_syllable_only() {
            assert!(match_full_pinyin("bi", &seg("匕首")));
        }

        #[test]
        fn test_fp_mismatch() {
            assert!(!match_full_pinyin("bishouxxx", &seg("匕首")));
        }

        #[test]
        fn test_fp_heteronym_chang() {
            assert!(match_full_pinyin("chang", &seg("长")));
        }

        #[test]
        fn test_fp_heteronym_zhang() {
            assert!(match_full_pinyin("zhang", &seg("长")));
        }

        #[test]
        fn test_fp_heteronym_in_compound() {
            assert!(match_full_pinyin("changgong", &seg("长弓")));
            assert!(match_full_pinyin("zhanggong", &seg("长弓")));
        }

        #[test]
        fn test_fp_too_long_query() {
            assert!(!match_full_pinyin("bishoubishou", &seg("匕首")));
        }

        #[test]
        fn test_fp_empty_query() {
            assert!(match_full_pinyin("", &seg("匕首")));
        }

        #[test]
        fn test_fp_boundary_constraint() {
            assert!(!match_full_pinyin("axue", &seg("大学")));
        }

        #[test]
        fn test_fp_partial_last_syllable_prefix() {
            assert!(match_full_pinyin("dax", &seg("大学")));
        }
    }

    mod toplevel {
        use super::super::{pinyin_match, Segment};

        fn seg(name: &str) -> Segment {
            Segment::from_name(name)
        }

        #[test]
        fn test_match_first_letter_path() {
            assert!(pinyin_match("bs", &seg("匕首")));
        }

        #[test]
        fn test_match_full_path() {
            assert!(pinyin_match("bishou", &seg("匕首")));
        }

        #[test]
        fn test_match_neither() {
            assert!(!pinyin_match("xyz", &seg("匕首")));
        }

        #[test]
        fn test_match_empty_query() {
            assert!(pinyin_match("", &seg("匕首")));
        }

        #[test]
        fn test_match_ambiguous_bi() {
            assert!(pinyin_match("bi", &seg("匕首")));
        }

        #[test]
        fn test_match_mixed_mode_forbidden() {
            assert!(!pinyin_match("bshou", &seg("匕首")));
        }

        #[test]
        fn test_match_partial_first_letter() {
            // "s" matches 首 at position 1
            assert!(pinyin_match("s", &seg("匕首")));
        }

        #[test]
        fn test_match_partial_full_pinyin() {
            // "shou" matches 首 at position 1
            assert!(pinyin_match("shou", &seg("匕首")));
        }

        #[test]
        fn test_match_partial_middle() {
            // "ci" matches 赐 at position 1 of 大赐福
            assert!(pinyin_match("ci", &seg("大赐福")));
        }

        #[test]
        fn test_match_partial_no_match() {
            assert!(!pinyin_match("xyz", &seg("大赐福")));
        }
    }

    mod start_only {
        use super::super::{pinyin_match_start, Segment};

        fn seg(name: &str) -> Segment {
            Segment::from_name(name)
        }

        #[test]
        fn test_start_matches_beginning() {
            assert!(pinyin_match_start("b", &seg("匕首")));
            assert!(pinyin_match_start("bs", &seg("匕首")));
            assert!(pinyin_match_start("bishou", &seg("匕首")));
        }

        #[test]
        fn test_start_rejects_non_beginning() {
            // "s" matches 首 only at position 1, not from start
            assert!(!pinyin_match_start("s", &seg("匕首")));
            assert!(!pinyin_match_start("shou", &seg("匕首")));
        }

        #[test]
        fn test_start_empty_query() {
            assert!(pinyin_match_start("", &seg("匕首")));
        }
    }

    mod segment {
        use super::super::*;

        #[test]
        fn test_segment_empty() {
            let seg = Segment::from_name("");
            assert!(seg.0.is_empty());
        }

        #[test]
        fn test_segment_single_cjk() {
            // 匕 pinyin is "bi"
            let seg = Segment::from_name("匕");
            assert_eq!(seg.0.len(), 1);
            assert!(seg.0[0].is_cjk);
            assert!(
                seg.0[0].readings.contains(&"bi"),
                "Expected 'bi' in readings, got: {:?}",
                seg.0[0].readings
            );
        }

        #[test]
        fn test_segment_single_heteronym() {
            // 长 is a polyphone with readings "chang" and "zhang"
            let seg = Segment::from_name("长");
            assert_eq!(seg.0.len(), 1);
            assert!(seg.0[0].is_cjk);
            assert!(
                seg.0[0].readings.contains(&"chang"),
                "Expected 'chang' in readings, got: {:?}",
                seg.0[0].readings
            );
            assert!(
                seg.0[0].readings.contains(&"zhang"),
                "Expected 'zhang' in readings, got: {:?}",
                seg.0[0].readings
            );
        }

        #[test]
        fn test_segment_multi_cjk() {
            // 匕首 = 2 CJK chars
            let seg = Segment::from_name("匕首");
            assert_eq!(seg.0.len(), 2);
            assert!(seg.0[0].readings.contains(&"bi"), "got: {:?}", seg.0[0].readings);
            assert!(seg.0[1].readings.contains(&"shou"), "got: {:?}", seg.0[1].readings);
        }

        #[test]
        fn test_segment_ascii_mixed() {
            // "A1武器" → 4 chars: 'a'(non-cjk), '1'(non-cjk), 武(cjk), 器(cjk)
            let seg = Segment::from_name("A1武器");
            assert_eq!(seg.0.len(), 4);
            assert!(!seg.0[0].is_cjk);
            assert!(!seg.0[1].is_cjk);
            assert!(seg.0[2].is_cjk);
            assert!(seg.0[3].is_cjk);
            assert_eq!(seg.0[0].ch, 'a'); // uppercased A → lowercased to 'a'
            assert_eq!(seg.0[1].ch, '1');
        }

        #[test]
        fn test_segment_bracket_prefix() {
            // "[圆桌厅堂]" → 6 chars: [, 圆, 桌, 厅, 堂, ]
            let seg = Segment::from_name("[圆桌厅堂]");
            assert_eq!(seg.0.len(), 6);
            assert!(!seg.0[0].is_cjk); // [
            assert!(seg.0[1].is_cjk); // 圆
            assert!(seg.0[2].is_cjk); // 桌
            assert!(seg.0[3].is_cjk); // 厅
            assert!(seg.0[4].is_cjk); // 堂
            assert!(!seg.0[5].is_cjk); // ]
        }

        #[test]
        fn test_segment_no_panic_on_special_chars() {
            // Should not panic on any character
            let _ = Segment::from_name("·"); // middle dot
            let _ = Segment::from_name("【】"); // fullwidth brackets
            let _ = Segment::from_name("+10"); // item upgrade notation
        }
    }

    mod integration {
        use serde_json::Value;

        use super::super::{pinyin_match, Segment};

        /// Parse item_ids.json and collect all leaf node names
        fn load_all_leaf_names() -> Vec<String> {
            let json_str = include_str!("item_ids.json");
            let data: Value =
                serde_json::from_str(json_str).expect("item_ids.json must be valid JSON");
            let mut names = Vec::new();
            collect_leaves(&data, &mut names);
            names
        }

        fn collect_leaves(val: &Value, acc: &mut Vec<String>) {
            match val {
                Value::Array(arr) => {
                    for item in arr {
                        collect_leaves(item, acc);
                    }
                },
                Value::Object(obj) => {
                    if obj.contains_key("value") {
                        // Leaf node: has "node" name and "value" id
                        if let Some(Value::String(name)) = obj.get("node") {
                            acc.push(name.clone());
                        }
                    } else if let Some(children) = obj.get("children") {
                        // Inner node: recurse into children
                        collect_leaves(children, acc);
                    }
                },
                _ => {},
            }
        }

        #[test]
        fn test_all_items_build_segments_no_panic() {
            let names = load_all_leaf_names();
            assert!(!names.is_empty(), "Expected items from item_ids.json, got 0");
            println!("Loaded {} item names", names.len());
            for name in &names {
                // This must not panic
                let _seg = Segment::from_name(name);
            }
        }

        #[test]
        fn test_real_items_bishou() {
            let names = load_all_leaf_names();
            let bishou_name = names
                .iter()
                .find(|n| n.as_str() == "匕首")
                .expect("'匕首' must exist in item_ids.json");
            let seg = Segment::from_name(bishou_name);

            assert!(pinyin_match("bs", &seg), "'bs' should match '匕首'");
            assert!(pinyin_match("bishou", &seg), "'bishou' should match '匕首'");
            assert!(
                pinyin_match("bish", &seg),
                "'bish' (partial last syllable) should match '匕首'"
            );
            assert!(pinyin_match("b", &seg), "'b' should match '匕首'");
            assert!(!pinyin_match("xyz", &seg), "'xyz' should NOT match '匕首'");
        }

        #[test]
        fn test_full_filter_performance() {
            let names = load_all_leaf_names();
            let segments: Vec<Segment> = names.iter().map(|n| Segment::from_name(n)).collect();

            let start = std::time::Instant::now();
            let count = segments.iter().filter(|s| pinyin_match("js", s)).count();
            let elapsed = start.elapsed();
            println!("'js' matched {} items in {:?}", count, elapsed);
            assert!(elapsed.as_millis() < 20, "Filter took too long: {:?} (budget: 20ms)", elapsed);
        }
    }
}
