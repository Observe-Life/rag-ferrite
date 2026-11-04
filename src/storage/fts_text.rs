//! 中文关键词切词：为 SQLite FTS5 准备可命中的文本表示。
//!
//! FTS5 的 `unicode61` 分词器按「非字母数字」切词，因此连续汉字会被当成
//! 一个词，中文关键词检索必然落空。本模块在写入索引和解析查询之前，把文本
//! 转成空格分隔的词元序列，交给 unicode61 正常切分。
//!
//! 规则（无词典、无特例表，全部是固定算法步骤）：
//! 1. 汉字段 → 每个单字 + 每个相邻双字。单字兜住一字查询，双字提供区分度。
//! 2. ASCII 字母数字段 → 整词并转小写，允许内部 `-` `_` `.` `/` `+`，
//!    使 `gpt-4o-mini`、`docker-compose.yml` 这类型号名与文件名不被切碎。
//! 3. 字母数字段与汉字段在原文中紧邻时 → 追加跨界词元，
//!    使 `B超`、`5G网络`、`2024年`、`维生素C` 能被整体命中。
//! 4. 其余字符（标点、空白、emoji）一律视为分隔符并断开，
//!    因此「海鲜、虾蟹」不会产生跨标点的假词元。
//!
//! 已知降级（为保持零依赖而明确接受）：
//! - 不做 NFKC 归一化，全角字母数字（如 `Ｘ４００`）与半角版本互不相通；
//! - 非 ASCII 字母（希腊字母等）按分隔符处理。
//!
//! 索引侧与查询侧必须调用同一个 [`index_text`]，两侧切法不一致会让检索
//! 静默失效且不报错，这是中文 BM25 最常见的坏法。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Ideograph,
    Word,
}

/// 词元内部允许的连接符。
const JOINERS: [char; 5] = ['-', '_', '.', '/', '+'];

fn class_of(c: char) -> Option<Class> {
    let o = c as u32;
    if matches!(o,
        0x3040..=0x30FF   // 日文假名
        | 0x3400..=0x4DBF // 汉字扩展 A
        | 0x4E00..=0x9FFF // 汉字基本区
        | 0xF900..=0xFAFF // 兼容汉字
        | 0xAC00..=0xD7AF // 韩文谚文
        | 0x20000..=0x2FA1F // 汉字扩展 B–F（生僻人名用字）
    ) {
        Some(Class::Ideograph)
    } else if c.is_ascii_alphanumeric() {
        Some(Class::Word)
    } else {
        None
    }
}

#[derive(Debug)]
struct Seg {
    kind: Class,
    text: String,
    start: usize,
    end: usize,
}

/// 把文本切成「汉字段」与「字母数字段」序列，其余字符作为分隔符丢弃。
fn segments(text: &str) -> Vec<Seg> {
    let chars: Vec<char> = text.chars().collect();
    let mut segs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match class_of(chars[i]) {
            Some(Class::Ideograph) => {
                let start = i;
                while i < chars.len() && class_of(chars[i]) == Some(Class::Ideograph) {
                    i += 1;
                }
                segs.push(Seg {
                    kind: Class::Ideograph,
                    text: chars[start..i].iter().collect(),
                    start,
                    end: i,
                });
            }
            Some(Class::Word) => {
                let start = i;
                while i < chars.len() {
                    if class_of(chars[i]) == Some(Class::Word) {
                        i += 1;
                        continue;
                    }
                    // 连接符仅在其后仍是字母数字时归入本词，避免把句点当词尾
                    if JOINERS.contains(&chars[i])
                        && i + 1 < chars.len()
                        && class_of(chars[i + 1]) == Some(Class::Word)
                    {
                        i += 1;
                        continue;
                    }
                    break;
                }
                segs.push(Seg {
                    kind: Class::Word,
                    text: chars[start..i].iter().collect::<String>().to_lowercase(),
                    start,
                    end: i,
                });
            }
            None => i += 1,
        }
    }
    segs
}

/// 汉字段：单字 + 相邻双字。
fn push_ideograph_terms(cs: &[char], terms: &mut Vec<String>) {
    for c in cs {
        terms.push(c.to_string());
    }
    for w in cs.windows(2) {
        terms.push(w.iter().collect());
    }
}

/// 字母数字段：整词 + 连接符子词 + 字母数字混写的字母前缀。
fn push_word_terms(word: &str, terms: &mut Vec<String>) {
    terms.push(word.to_owned());

    if word.contains(JOINERS) {
        for part in word.split(JOINERS) {
            if part.is_empty() {
                continue;
            }
            let digits = part.chars().all(|c| c.is_ascii_digit());
            if part.chars().count() >= 2 || digits {
                terms.push(part.to_owned());
            }
        }
    }

    // iphone15 → 追加 iphone 与 15，使不带型号数字的查询也能命中
    if let Some(pos) = word.find(|c: char| c.is_ascii_digit()) {
        let (alpha, num) = word.split_at(pos);
        if alpha.chars().count() >= 3 && !num.is_empty() {
            terms.push(alpha.to_owned());
            terms.push(num.to_owned());
        }
    }
}

/// 索引与查询共用的唯一切词函数，返回空格分隔的词元串。
pub fn index_text(text: &str) -> String {
    let segs = segments(text);
    let mut terms: Vec<String> = Vec::new();

    for (idx, seg) in segs.iter().enumerate() {
        let cur: Vec<char> = seg.text.chars().collect();
        match seg.kind {
            Class::Ideograph => push_ideograph_terms(&cur, &mut terms),
            Class::Word => push_word_terms(&seg.text, &mut terms),
        }

        // 紧邻（中间没有分隔符）的异类段 → 追加跨界词元
        if let Some(next) = segs.get(idx + 1) {
            if next.start != seg.end || next.kind == seg.kind {
                continue;
            }
            let nxt: Vec<char> = next.text.chars().collect();
            match (seg.kind, next.kind) {
                (Class::Word, Class::Ideograph) => {
                    if let Some(c) = nxt.first() {
                        terms.push(format!("{}{}", seg.text, c));
                    }
                    if nxt.len() >= 2 {
                        terms.push(format!("{}{}", seg.text, nxt[..2].iter().collect::<String>()));
                    }
                }
                (Class::Ideograph, Class::Word) => {
                    if let Some(c) = cur.last() {
                        terms.push(format!("{}{}", c, next.text));
                    }
                    if cur.len() >= 2 {
                        terms.push(format!(
                            "{}{}",
                            cur[cur.len() - 2..].iter().collect::<String>(),
                            next.text
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    terms.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(text: &str) -> Vec<String> {
        index_text(text).split(' ').filter(|s| !s.is_empty()).collect()
    }

    #[test]
    fn ideographs_yield_unigrams_and_bigrams() {
        let t = terms("海鲜过敏");
        assert!(t.contains(&"海".to_owned()));
        assert!(t.contains(&"海鲜".to_owned()));
        assert!(t.contains(&"鲜过".to_owned()));
        assert!(t.contains(&"过敏".to_owned()));
    }

    #[test]
    fn punctuation_breaks_runs_so_no_fake_bigram_spans_it() {
        let t = terms("海鲜、虾蟹");
        assert!(!t.contains(&"鲜虾".to_owned()), "跨顿号的假词元: {t:?}");
        assert!(t.contains(&"虾蟹".to_owned()));
    }

    #[test]
    fn latin_runs_keep_identifiers_whole() {
        let t = terms("gpt-4o-mini");
        assert!(t.contains(&"gpt-4o-mini".to_owned()));
        assert!(t.contains(&"gpt".to_owned()));
        assert!(t.contains(&"mini".to_owned()));
    }

    #[test]
    fn mixed_runs_add_boundary_terms() {
        assert!(terms("B超").contains(&"b超".to_owned()));
        assert!(terms("5G网络").contains(&"5g网络".to_owned()));
        assert!(terms("2024年").contains(&"2024年".to_owned()));
        assert!(terms("维生素C").contains(&"素c".to_owned()));
    }

    #[test]
    fn alnum_prefix_is_extracted() {
        let t = terms("iPhone15手机");
        assert!(t.contains(&"iphone".to_owned()));
        assert!(t.contains(&"iphone15".to_owned()));
    }

    #[test]
    fn case_is_normalized() {
        assert_eq!(terms("CLIPPY.TOML").contains(&"clippy.toml".to_owned()), true);
    }

    #[test]
    fn index_and_query_use_the_same_function() {
        // 查询词必须能在包含它的文档切法中找到，否则检索会静默失效
        let doc = terms("我对海鲜过敏，尤其是虾蟹");
        for query_term in terms("虾蟹") {
            assert!(doc.contains(&query_term), "查询词元 {query_term:?} 不在文档切法中");
        }
    }
}
