//! 记录时间线的口径（DESIGN §2.5）：分组、搜索、标签筛选只写这一处，前端只渲染。
//!
//! 分组键是 `Note::local_date`——04:00 日界在写入时就算好了，凌晨两点写的那条属于前一晚，
//! 这里不再按 `createdAt` 的日历日重切一遍，否则两端会切出两份不一样的时间线。
//!
//! 搜索把词按空白切开、全部命中才算（「读书 雨天」是两件事都要有）；以 `#` 开头的词只查标签，
//! 正文里正好出现的同名子串不算。命中哪些区间也在这里算好（按字符，不按字节），
//! 前端只负责把这几段包进 `<mark>`，不在 JS 里再判一次大小写。

use std::collections::HashMap;

use serde::Serialize;

use crate::date_boundary::{parse_date, parse_instant};
use crate::habits::is_tombstoned;
use crate::model::{Data, EntityType, Note};

/// 「以前的一天」至少隔多少天才算：上周的记录在时间线第一屏就看得见，不算回顾。
const MEMORY_MIN_DAYS: i64 = 7;

/// 时间线上的一条记录。`time_of_day` 是写下它的那一刻（几点几分），用来区分同一天的多条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRow {
    pub id: String,
    pub created_at: String,
    pub time_of_day: String,
    pub text: String,
    pub tags: Vec<String>,
    /// 正文里命中的字符区间（不是字节下标），前端只负责把它们包进 `<mark>`。
    pub text_hits: Vec<(usize, usize)>,
    /// 与 `tags` 一一对应：这个标签是否被搜索词命中。
    pub tag_hits: Vec<bool>,
    /// 同步冲突副本：界面给一个轻量标识，不隐藏也不合并（§4.5）。
    pub conflict: bool,
}

/// 「以前的一天」要展示的那条记录。同一天里始终挑同一条（§2.5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub local_date: String,
    pub text: String,
    pub tags: Vec<String>,
    pub conflict: bool,
}

/// 一天的记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayGroup {
    pub local_date: String,
    pub notes: Vec<NoteRow>,
}

/// 标签筛选的候选与它的条数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

/// 搜索词切出来的一个词。`tag_only` 是 `#` 开头的那些，只去标签里找。
struct Token {
    text: String,
    tag_only: bool,
}

/// 这条记录在当前筛选下要不要显示。`text` 是搜索词，`tag` 是选中的标签，空串都表示不设限。
///
/// 每个词只做字面包含（不做语义），多个词之间是「且」；普通词正文和标签两边都收，
/// 正文里 `#工作` 与标签里的「工作」算同一个东西，因为存进去的正文本来就带着 `#`。
pub fn matches(note: &Note, text: &str, tag: &str) -> bool {
    matches_tokens(note, &tokens(text), tag)
}

fn matches_tokens(note: &Note, tokens: &[Token], tag: &str) -> bool {
    if !tokens.is_empty() {
        let body = note.text.to_lowercase();
        let tags: Vec<String> = note.tags.iter().map(|t| t.to_lowercase()).collect();
        for token in tokens {
            let in_tag = tags.iter().any(|t| t.contains(&token.text));
            if token.tag_only {
                if !in_tag {
                    return false;
                }
            } else if !in_tag && !body.contains(&token.text) {
                return false;
            }
        }
    }
    let wanted = tag.trim().trim_start_matches('#').to_lowercase();
    if wanted.is_empty() {
        return true;
    }
    note.tags
        .iter()
        .any(|t| t.to_lowercase().trim_start_matches('#') == wanted)
}

/// 时间线：按 localDate 倒序分组，组内按写入时刻倒序。墓碑记录不出现。
pub fn timeline(data: &Data, text: &str, tag: &str) -> Vec<DayGroup> {
    let tokens = tokens(text);
    let mut days: HashMap<String, Vec<&Note>> = HashMap::new();
    for note in &data.notes {
        if is_tombstoned(data, EntityType::Note, &note.id) || !matches_tokens(note, &tokens, tag) {
            continue;
        }
        days.entry(note.local_date.clone()).or_default().push(note);
    }
    let mut keys: Vec<String> = days.keys().cloned().collect();
    keys.sort_by_key(|d| std::cmp::Reverse(parse_date(d).unwrap_or(i64::MIN)));
    keys.into_iter()
        .map(|day| {
            let mut notes: Vec<&Note> = days.remove(&day).unwrap_or_default();
            notes.sort_by_key(|n| std::cmp::Reverse(instant(n)));
            DayGroup {
                local_date: day,
                notes: notes.into_iter().map(|note| row(note, &tokens)).collect(),
            }
        })
        .collect()
}

/// 「以前的一天」：从至少七天前的存活记录里挑一条，挑法只由日期决定——
/// 同一天里刷新多少次都是同一条，两台设备数据一样时也挑到同一条。
pub fn daily_memory(data: &Data, today: &str) -> Option<Memory> {
    let today_days = parse_date(today)?;
    let mut pool: Vec<&Note> = data
        .notes
        .iter()
        .filter(|note| !is_tombstoned(data, EntityType::Note, &note.id))
        .filter(|note| {
            parse_date(&note.local_date).is_some_and(|day| today_days - day >= MEMORY_MIN_DAYS)
        })
        .collect();
    if pool.is_empty() {
        return None;
    }
    // 排序只为让下标稳定，不表达「越旧越优先」。
    pool.sort_by(|a, b| {
        a.local_date
            .cmp(&b.local_date)
            .then_with(|| a.id.cmp(&b.id))
    });
    let pick = (fnv1a(today) % pool.len() as u64) as usize;
    let note = pool[pick];
    Some(Memory {
        local_date: note.local_date.clone(),
        text: note.text.clone(),
        tags: note.tags.clone(),
        conflict: note.conflict_of.is_some(),
    })
}

/// 出现过的标签，按条数从多到少、同数按名字排；筛选候选用它，不在前端重算。
pub fn tag_counts(data: &Data) -> Vec<TagCount> {
    let live: Vec<&Note> = data
        .notes
        .iter()
        .filter(|note| !is_tombstoned(data, EntityType::Note, &note.id))
        .collect();
    tag_counts_from(&live)
}

/// 同一套计数与排序，作用在调用方已经挑好的那批记录上——统计里的 Top 5 就是这么来的（§6.3）。
/// 排序规则只写这一处，两处的「最常写到」才不会出现两个答案。
pub fn tag_counts_from(notes: &[&Note]) -> Vec<TagCount> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for note in notes {
        for tag in &note.tags {
            *counts.entry(tag.trim().to_string()).or_default() += 1;
        }
    }
    let mut out: Vec<TagCount> = counts
        .into_iter()
        .filter(|(tag, _)| !tag.is_empty())
        .map(|(tag, count)| TagCount { tag, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    out
}

/// 搜索词按空白切词；`#` 开头去掉井号后只查标签，井号后面空掉的词忽略。
fn tokens(query: &str) -> Vec<Token> {
    query
        .split_whitespace()
        .filter_map(|raw| match raw.strip_prefix('#') {
            Some(rest) => {
                let text = rest.trim().to_lowercase();
                (!text.is_empty()).then_some(Token {
                    text,
                    tag_only: true,
                })
            }
            None => Some(Token {
                text: raw.to_lowercase(),
                tag_only: false,
            }),
        })
        .collect()
}

fn row(note: &Note, tokens: &[Token]) -> NoteRow {
    let mut hits = Vec::new();
    for token in tokens {
        // `#` 词在正文里认的是整段 `#标签`：光秃秃的同名子串不是这次搜索要找的东西。
        let needle = if token.tag_only {
            format!("#{}", token.text)
        } else {
            token.text.clone()
        };
        find_all(&note.text, &needle, &mut hits);
    }
    let tag_hits = note
        .tags
        .iter()
        .map(|tag| {
            let folded = tag.to_lowercase();
            tokens.iter().any(|token| folded.contains(&token.text))
        })
        .collect();
    NoteRow {
        id: note.id.clone(),
        created_at: note.created_at.clone(),
        time_of_day: clock(&note.created_at),
        text: note.text.clone(),
        tags: note.tags.clone(),
        text_hits: merge(hits),
        tag_hits,
        conflict: note.conflict_of.is_some(),
    }
}

/// 把 `needle` 在 `text` 里出现的位置按字符区间收进 `out`。`needle` 必须已经小写折叠。
fn find_all(text: &str, needle: &str, out: &mut Vec<(usize, usize)>) {
    if needle.is_empty() {
        return;
    }
    let (folded, owner) = fold(text);
    for (start, _) in folded.match_indices(needle) {
        let last = start + needle.len() - 1;
        out.push((owner[start], owner[last] + 1));
    }
}

/// 小写折叠，并记住每个折叠后字节来自原串的第几个字符。
///
/// 高亮区间按字符算，前端按码点切：两端都从同一份折叠结果出发，
/// 不用各自猜一次「ß 展开成 ss 之后下标怎么对」。
fn fold(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::new();
    let mut owner = Vec::new();
    for (index, ch) in text.chars().enumerate() {
        for lower in ch.to_lowercase() {
            let before = folded.len();
            folded.push(lower);
            owner.extend(std::iter::repeat_n(index, folded.len() - before));
        }
    }
    (folded, owner)
}

/// 相邻或重叠的区间合成一段，免得 `<mark>` 互相嵌套。
fn merge(mut hits: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    hits.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(hits.len());
    for (start, end) in hits {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// FNV-1a：这里只要求「同一天一定得到同一个下标」，不用于安全用途。
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 写入时刻的「HH:MM」。读不出来的时间戳留空串，界面就不显示时刻，不编一个。
fn clock(iso: &str) -> String {
    let Some(rest) = iso.get(11..16) else {
        return String::new();
    };
    let ok = rest.chars().enumerate().all(|(i, c)| match i {
        2 => c == ':',
        _ => c.is_ascii_digit(),
    });
    if ok {
        rest.to_string()
    } else {
        String::new()
    }
}

fn instant(note: &Note) -> i64 {
    parse_instant(&note.created_at).unwrap_or(i64::MIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Tombstone;

    fn note(id: &str, day: &str, at: &str, text: &str, tags: &[&str]) -> Note {
        Note {
            id: id.to_string(),
            created_at: format!("{at}.000+08:00"),
            local_date: day.to_string(),
            text: text.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            updated_at: format!("{at}.000+08:00"),
            conflict_of: None,
        }
    }

    fn data_with(notes: Vec<Note>) -> Data {
        let mut data = Data::empty("dev-a");
        data.notes = notes;
        data
    }

    fn three_notes() -> Vec<Note> {
        vec![
            note(
                "n1",
                "2026-10-26",
                "2026-10-26T21:05",
                "读完了两章 #阅读",
                &["阅读"],
            ),
            note("n2", "2026-10-26", "2026-10-26T09:12", "早上背了单词", &[]),
            note(
                "n3",
                "2026-10-25",
                "2026-10-25T23:40",
                "只写了一行 #阅读",
                &["阅读"],
            ),
        ]
    }

    #[test]
    fn groups_run_newest_day_first_and_newest_note_first() {
        let groups = timeline(&data_with(three_notes()), "", "");
        let days: Vec<&str> = groups.iter().map(|g| g.local_date.as_str()).collect();
        assert_eq!(days, vec!["2026-10-26", "2026-10-25"]);
        assert_eq!(
            groups[0]
                .notes
                .iter()
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            vec!["n1", "n2"],
            "同一天里后写的排前面"
        );
    }

    #[test]
    fn grouping_follows_local_date_not_the_calendar_day() {
        // 凌晨 02:30 写的那条属于前一晚：分组看 localDate，不看 createdAt 的日期。
        let data = data_with(vec![note(
            "n1",
            "2026-10-26",
            "2026-10-27T02:30",
            "半夜想起来的事",
            &[],
        )]);
        let groups = timeline(&data, "", "");
        assert_eq!(groups[0].local_date, "2026-10-26");
        assert_eq!(groups[0].notes[0].time_of_day, "02:30", "时刻仍然显示凌晨");
    }

    #[test]
    fn search_matches_body_and_tags_without_caring_about_case() {
        let data = data_with(vec![
            note(
                "n1",
                "2026-10-26",
                "2026-10-26T21:05",
                "Read a chapter",
                &["阅读"],
            ),
            note(
                "n2",
                "2026-10-26",
                "2026-10-26T21:06",
                "散步半小时",
                &["Walk"],
            ),
        ]);
        assert_eq!(ids(&timeline(&data, "read", "")), vec!["n1"]);
        assert_eq!(ids(&timeline(&data, "READ", "")), vec!["n1"]);
        assert_eq!(
            ids(&timeline(&data, "walk", "")),
            vec!["n2"],
            "标签也算命中"
        );
        assert!(timeline(&data, "没有这个词", "").is_empty());
    }

    #[test]
    fn search_requires_every_word() {
        let data = data_with(vec![
            note(
                "n1",
                "2026-10-26",
                "2026-10-26T21:05",
                "读完两章 #阅读",
                &["阅读"],
            ),
            note("n2", "2026-10-25", "2026-10-25T21:05", "雨天散步", &[]),
        ]);
        assert_eq!(ids(&timeline(&data, "读完 两章", "")), vec!["n1"]);
        assert!(
            timeline(&data, "读完 雨天", "").is_empty(),
            "有一个词落空就不算命中"
        );
        assert_eq!(
            ids(&timeline(&data, "阅读 两章", "")),
            vec!["n1"],
            "一个词命中标签、另一个命中正文也算"
        );
    }

    #[test]
    fn hash_token_searches_tags_only() {
        let data = data_with(vec![note(
            "n1",
            "2026-10-26",
            "2026-10-26T21:05",
            "书里夹着一张票根",
            &["阅读"],
        )]);
        assert_eq!(ids(&timeline(&data, "#阅读", "")), vec!["n1"]);
        assert!(
            timeline(&data, "#书", "").is_empty(),
            "正文里的同名子串不算标签"
        );
        assert_eq!(
            ids(&timeline(&data, "书", "")),
            vec!["n1"],
            "不加井号就查正文"
        );
        assert_eq!(
            ids(&timeline(&data, "#", "")),
            vec!["n1"],
            "只有井号等于什么都没筛"
        );
    }

    #[test]
    fn hits_are_character_ranges_and_tags_line_up() {
        let data = data_with(vec![note(
            "n1",
            "2026-10-26",
            "2026-10-26T21:05",
            "读了两章📖",
            &["阅读"],
        )]);
        let group = timeline(&data, "读 阅读", "");
        let row = &group[0].notes[0];
        assert_eq!(row.text_hits, vec![(0, 1)], "正文只标出正文里真有的那个词");
        assert_eq!(
            row.tag_hits,
            vec![true],
            "命中标签只亮标签，不把正文里没有的字点亮"
        );
        let group = timeline(&data, "📖", "");
        assert_eq!(
            group[0].notes[0].text_hits,
            vec![(4, 5)],
            "emoji 按一个字符算，前端按码点切才对得上"
        );
    }

    #[test]
    fn overlapping_hits_are_merged_once() {
        let data = data_with(vec![note(
            "n1",
            "2026-10-26",
            "2026-10-26T21:05",
            "读书读书",
            &[],
        )]);
        let row = &timeline(&data, "读书 书读", "")[0].notes[0];
        assert_eq!(row.text_hits, vec![(0, 4)], "重叠和相邻都合成一段");
    }

    #[test]
    fn hits_follow_the_same_case_folding_as_matching() {
        let data = data_with(vec![note(
            "n1",
            "2026-10-26",
            "2026-10-26T21:05",
            "Read a chapter",
            &[],
        )]);
        let row = &timeline(&data, "READ", "")[0].notes[0];
        assert_eq!(row.text_hits, vec![(0, 4)]);
    }

    #[test]
    fn conflict_copies_carry_a_quiet_mark_in_the_timeline_and_the_memory() {
        let mut copy = note("n-copy", "2026-09-20", "2026-09-20T20:00", "同一句话", &[]);
        copy.conflict_of = Some("n1".to_string());
        let data = data_with(vec![copy]);
        let row = &timeline(&data, "", "")[0].notes[0];
        assert!(row.conflict);
        let memory = daily_memory(&data, "2026-10-26").expect("有够旧的记录");
        assert!(memory.conflict, "今天抽到副本时也要看得出来");
    }

    #[test]
    fn memory_keeps_to_notes_at_least_a_week_old() {
        let data = data_with(vec![
            note("n1", "2026-10-26", "2026-10-26T21:05", "昨天", &[]),
            note("n2", "2026-10-20", "2026-10-20T21:05", "六天前", &[]),
            note("n3", "2026-10-19", "2026-10-19T21:05", "七天前", &[]),
            note("n4", "2025-12-01", "2025-12-01T21:05", "去年", &[]),
        ]);
        let memory = daily_memory(&data, "2026-10-26").expect("有七天前的记录");
        assert!(
            matches!(memory.local_date.as_str(), "2026-10-19" | "2025-12-01"),
            "上周和昨天都不算「以前的一天」"
        );
    }

    #[test]
    fn memory_is_the_same_note_all_day_and_does_not_depend_on_insert_order() {
        let notes = vec![
            note("n1", "2026-09-01", "2026-09-01T21:05", "九月", &[]),
            note("n2", "2026-09-15", "2026-09-15T21:05", "九月中", &[]),
            note("n3", "2026-10-01", "2026-10-01T21:05", "十月", &[]),
        ];
        let forward = data_with(notes.clone());
        let mut backward_notes = notes;
        backward_notes.reverse();
        let backward = data_with(backward_notes);
        let first = daily_memory(&forward, "2026-10-26");
        assert_eq!(first, daily_memory(&forward, "2026-10-26"));
        assert_eq!(
            first,
            daily_memory(&backward, "2026-10-26"),
            "两端合并进来的顺序不同，挑到的仍是同一条"
        );
    }

    #[test]
    fn memory_returns_nothing_when_the_old_notes_are_gone() {
        let mut data = data_with(vec![note(
            "n1",
            "2026-10-01",
            "2026-10-01T21:05",
            "旧事",
            &[],
        )]);
        assert!(daily_memory(&data, "2026-10-26").is_some());
        data.tombstones.push(Tombstone {
            entity_type: EntityType::Note,
            entity_id: "n1".to_string(),
            deleted_at: "2026-10-27T10:00:00.000+08:00".to_string(),
            deleted_by: "dev-a".to_string(),
        });
        assert!(daily_memory(&data, "2026-10-26").is_none());
        let recent = data_with(vec![note(
            "n2",
            "2026-10-24",
            "2026-10-24T21:05",
            "刚记的",
            &[],
        )]);
        assert!(
            daily_memory(&recent, "2026-10-26").is_none(),
            "没有更旧的记录就宁可空着，不拿近几天的顶替"
        );
    }

    #[test]
    fn tag_filter_keeps_only_that_tag_and_accepts_the_hash() {
        let data = data_with(three_notes());
        assert_eq!(ids(&timeline(&data, "", "阅读")), vec!["n1", "n3"]);
        assert_eq!(
            ids(&timeline(&data, "", "#阅读")),
            vec!["n1", "n3"],
            "输入框里带 # 也算同一个标签"
        );
        assert!(timeline(&data, "", "散步").is_empty());
    }

    #[test]
    fn a_tombstoned_note_is_gone_from_every_view_of_the_timeline() {
        let mut data = data_with(three_notes());
        data.tombstones.push(Tombstone {
            entity_type: EntityType::Note,
            entity_id: "n1".to_string(),
            deleted_at: "2026-10-27T10:00:00.000+08:00".to_string(),
            deleted_by: "dev-a".to_string(),
        });
        assert_eq!(ids(&timeline(&data, "", "")), vec!["n2", "n3"]);
        assert_eq!(
            ids(&timeline(&data, "两章", "")),
            Vec::<&str>::new(),
            "搜索也不该把它捞回来"
        );
        assert_eq!(tag_counts(&data)[0].count, 1, "标签计数同样不算已删除的");
    }

    #[test]
    fn tag_counts_sort_by_size_then_name() {
        let data = data_with(vec![
            note(
                "n1",
                "2026-10-26",
                "2026-10-26T08:00",
                "#工作 #课堂",
                &["工作", "课堂"],
            ),
            note("n2", "2026-10-25", "2026-10-25T08:00", "#工作", &["工作"]),
            note("n3", "2026-10-24", "2026-10-24T08:00", "", &[]),
        ]);
        let tags = tag_counts(&data);
        assert_eq!(
            tags.iter()
                .map(|t| (t.tag.as_str(), t.count))
                .collect::<Vec<_>>(),
            vec![("工作", 2), ("课堂", 1)]
        );
    }

    #[test]
    fn an_unreadable_timestamp_shows_no_clock_instead_of_a_made_up_one() {
        assert_eq!(clock("2026-10-26T21:05:00.000+08:00"), "21:05");
        assert_eq!(clock("昨天"), "");
        assert_eq!(clock("2026-10-26T晚上"), "");
    }

    fn ids(groups: &[DayGroup]) -> Vec<&str> {
        groups
            .iter()
            .flat_map(|g| &g.notes)
            .map(|n| n.id.as_str())
            .collect()
    }
}
