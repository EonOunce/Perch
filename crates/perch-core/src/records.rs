//! 记录时间线的口径（DESIGN §2.5）：分组、搜索、标签筛选只写这一处，前端只渲染。
//!
//! 分组键是 `Note::local_date`——04:00 日界在写入时就算好了，凌晨两点写的那条属于前一晚，
//! 这里不再按 `createdAt` 的日历日重切一遍，否则两端会切出两份不一样的时间线。

use std::collections::HashMap;

use serde::Serialize;

use crate::date_boundary::{parse_date, parse_instant};
use crate::habits::is_tombstoned;
use crate::model::{Data, EntityType, Note};

/// 时间线上的一条记录。`time_of_day` 是写下它的那一刻（几点几分），用来区分同一天的多条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRow {
    pub id: String,
    pub created_at: String,
    pub time_of_day: String,
    pub text: String,
    pub tags: Vec<String>,
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

/// 这条记录在当前筛选下要不要显示。`text` 是搜索词，`tag` 是选中的标签，空串都表示不设限。
///
/// 搜索只做字面包含（不分词、不做语义），标签两边都收：正文里 `#工作` 与标签里的「工作」
/// 算同一个东西，因为存进去的正文本来就带着 `#`。
pub fn matches(note: &Note, text: &str, tag: &str) -> bool {
    let needle = text.trim().to_lowercase();
    if !needle.is_empty() {
        let body = note.text.to_lowercase();
        let tags = note.tags.iter().any(|t| t.to_lowercase().contains(&needle));
        if !body.contains(&needle) && !tags {
            return false;
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
    let mut days: HashMap<String, Vec<&Note>> = HashMap::new();
    for note in &data.notes {
        if is_tombstoned(data, EntityType::Note, &note.id) || !matches(note, text, tag) {
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
                notes: notes.into_iter().map(row).collect(),
            }
        })
        .collect()
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

fn row(note: &Note) -> NoteRow {
    NoteRow {
        id: note.id.clone(),
        created_at: note.created_at.clone(),
        time_of_day: clock(&note.created_at),
        text: note.text.clone(),
        tags: note.tags.clone(),
    }
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
