//! 导出口径（DESIGN §2.5、§4.6、§6.5）：把已经落盘的事实摊成可读的 Markdown。
//!
//! 这里不重新算统计，也不判断哪一天算「值得留下」——记录按日分组、心情取当天那条、
//! 周回顾直接用 [`crate::stats::Review`] 已经写好的事实句。保存到哪个文件夹、
//! 文件叫什么名字由宿主决定：`perch-core` 不碰用户目录（§5.4）。

use crate::date_boundary::{parse_date, parse_instant, weekday_mon_first};
use crate::habits::is_tombstoned;
use crate::model::{Data, EntityType, Note};
use crate::stats::Review;

const WEEKDAYS: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

/// 一条记录在 Markdown 里的形态：`- 21:05 正文`，多行正文缩进成列表里的续行。
///
/// 续行缩进四格而不是两格：两格的 `# 一行` 会被当成标题，四格算代码块，
/// 至少不会把用户自己写的井号变成文档结构。
fn note_lines(note: &Note, out: &mut Vec<String>) {
    let mut body = note.text.lines();
    let first = body.next().unwrap_or("").trim_end();
    let time = clock(&note.created_at);
    let head = if time.is_empty() {
        format!("- {first}")
    } else {
        format!("- {time} {first}")
    };
    out.push(if note.conflict_of.is_some() {
        format!("{head}（副本）")
    } else {
        head
    });
    for line in body {
        out.push(format!("    {line}"));
    }
}

/// 记录导出：按 `localDate` 倒序，同一天里后写的在前；每天先写心情，再写记录。
///
/// `now_label` 是「导出时间」那一行，由宿主按本地时区格式化（core 不读时钟）。
pub fn notes_markdown(data: &Data, now_label: &str) -> String {
    let mut lines = vec![
        "# 栖迟记录".to_string(),
        String::new(),
        format!("导出时间：{now_label}"),
    ];

    let mut notes: Vec<&Note> = data
        .notes
        .iter()
        .filter(|note| !is_tombstoned(data, EntityType::Note, &note.id))
        .collect();
    if notes.is_empty() {
        lines.push(String::new());
        lines.push("还没有记录。".to_string());
        return finish(lines);
    }

    let moods = live_moods(data);
    let mut days: Vec<String> = notes.iter().map(|note| note.local_date.clone()).collect();
    days.sort_by(|a, b| match (parse_date(a), parse_date(b)) {
        (Some(a), Some(b)) => b.cmp(&a),
        _ => b.cmp(a),
    });
    days.dedup();
    lines.push(format!("共 {} 天，{} 条记录。", days.len(), notes.len()));

    // 同一天里按写入时刻倒序；时刻相同按 id 反序，两端导出同一份数据会得到同一份文本。
    notes.sort_by(|a, b| instant(b).cmp(&instant(a)).then_with(|| b.id.cmp(&a.id)));

    for day in days {
        lines.push(String::new());
        lines.push(format!("## {day} {}", weekday(&day)));
        if let Some(score) = moods.get(&day) {
            lines.push(String::new());
            lines.push(format!("心情：{score} / 100"));
        }
        lines.push(String::new());
        for note in notes.iter().filter(|note| note.local_date == day) {
            note_lines(note, &mut lines);
        }
    }
    finish(lines)
}

/// 每周回顾的导出形态。`lines` 是规则生成的事实句，编辑发生在导出前，
/// 所以这份文本只是「把已经定稿的句子装进 Markdown」，不改写其中任何一个数。
pub fn review_markdown(review: &Review) -> String {
    let mut lines = vec!["# 栖迟 · 上周回顾".to_string(), String::new()];
    if !review.week_start.is_empty() {
        lines.push(format!("区间：{} – {}", review.week_start, review.week_end));
        lines.push(String::new());
    }
    lines.push("## 回顾".to_string());
    lines.push(String::new());
    for line in &review.lines {
        lines.push(format!("- {line}"));
    }
    lines.push(String::new());
    lines.push("> 由栖迟按固定规则生成：只有完成、目标和记录里的事实，不做评价。".to_string());
    finish(lines)
}

/// 一天最多一条心情是写入侧的自然键；万一读到重复，取最后改过的那条，导出结果仍然确定。
fn live_moods(data: &Data) -> std::collections::HashMap<String, i32> {
    let mut out = std::collections::HashMap::new();
    let mut stamps: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for mood in &data.daily_moods {
        if is_tombstoned(data, EntityType::DailyMood, &mood.id) || !(0..=100).contains(&mood.score)
        {
            continue;
        }
        match stamps.get(&mood.local_date) {
            Some(stamp) if stamp >= &mood.updated_at => {}
            _ => {
                stamps.insert(mood.local_date.clone(), mood.updated_at.clone());
                out.insert(mood.local_date.clone(), mood.score);
            }
        }
    }
    out
}

fn weekday(date: &str) -> &'static str {
    weekday_mon_first(date).map_or("", |index| WEEKDAYS[index as usize])
}

fn instant(note: &Note) -> i64 {
    parse_instant(&note.created_at).unwrap_or(i64::MIN)
}

/// 与 `records` 同一个取时刻的口径：读不出来就留空，不编一个。
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

/// 末尾永远只有一个换行：文本文件多一个空行没意义，少一个又有些编辑器会补。
fn finish(mut lines: Vec<String>) -> String {
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DailyMood, Tombstone};

    fn note(id: &str, day: &str, at: &str, text: &str) -> Note {
        Note {
            id: id.to_string(),
            created_at: format!("{at}.000+08:00"),
            local_date: day.to_string(),
            text: text.to_string(),
            tags: Vec::new(),
            updated_at: format!("{at}.000+08:00"),
            conflict_of: None,
        }
    }

    fn mood(id: &str, day: &str, score: i32) -> DailyMood {
        DailyMood {
            id: id.to_string(),
            local_date: day.to_string(),
            score,
            updated_at: format!("{day}T08:00:00.000+08:00"),
        }
    }

    #[test]
    fn notes_come_out_newest_day_first_and_newest_note_first() {
        let mut data = Data::empty("dev-a");
        data.notes = vec![
            note("a", "2026-10-25", "2026-10-25T23:40", "前一天晚上"),
            note("b", "2026-10-26", "2026-10-26T09:12", "早上"),
            note("c", "2026-10-26", "2026-10-26T21:05", "晚上"),
        ];
        let text = notes_markdown(&data, "2026-10-28 14:05");
        let days: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("## "))
            .collect();
        assert_eq!(
            days,
            vec!["## 2026-10-26 周一", "## 2026-10-25 周日"],
            "按日倒序，日期后带星期"
        );
        assert!(
            text.find("21:05 晚上").unwrap() < text.find("09:12 早上").unwrap(),
            "同一天后写的在前"
        );
        assert!(text.contains("共 2 天，3 条记录。"));
        assert!(text.starts_with("# 栖迟记录\n"));
        assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
    }

    #[test]
    fn mood_and_conflict_copies_are_marked_without_rewriting_the_note() {
        let mut data = Data::empty("dev-a");
        data.notes = vec![
            note("a", "2026-10-26", "2026-10-26T21:05", "读了两章 #阅读"),
            Note {
                conflict_of: Some("origin".to_string()),
                ..note("b", "2026-10-26", "2026-10-26T22:00", "同一个 id 的另一版")
            },
        ];
        data.daily_moods = vec![mood("m1", "2026-10-26", 62)];
        let text = notes_markdown(&data, "2026-10-28 14:05");
        assert!(text.contains("心情：62 / 100"));
        assert!(text.contains("- 22:00 同一个 id 的另一版（副本）"));
        assert!(
            text.contains("读了两章 #阅读"),
            "正文一个字都不改，井号标签原样保留"
        );
    }

    #[test]
    fn a_multiline_note_stays_one_list_item_and_cannot_become_a_heading() {
        let mut data = Data::empty("dev-a");
        data.notes = vec![note(
            "a",
            "2026-10-26",
            "2026-10-26T21:05",
            "第一行\n# 这不该变成标题\n第三行",
        )];
        let text = notes_markdown(&data, "2026-10-28 14:05");
        assert!(text.contains("- 21:05 第一行\n    # 这不该变成标题\n    第三行"));
    }

    #[test]
    fn deleted_notes_and_moods_stay_out_of_the_export() {
        let mut data = Data::empty("dev-a");
        data.notes = vec![
            note("a", "2026-10-26", "2026-10-26T21:05", "留下的"),
            note("b", "2026-10-26", "2026-10-26T22:05", "删掉的"),
        ];
        data.daily_moods = vec![mood("m1", "2026-10-26", 30)];
        data.tombstones = vec![
            Tombstone {
                entity_type: EntityType::Note,
                entity_id: "b".to_string(),
                deleted_at: "2026-10-27T00:00:00.000+08:00".to_string(),
                deleted_by: "dev-a".to_string(),
            },
            Tombstone {
                entity_type: EntityType::DailyMood,
                entity_id: "m1".to_string(),
                deleted_at: "2026-10-27T00:00:00.000+08:00".to_string(),
                deleted_by: "dev-a".to_string(),
            },
        ];
        let text = notes_markdown(&data, "2026-10-28 14:05");
        assert!(text.contains("留下的"));
        assert!(!text.contains("删掉的"));
        assert!(!text.contains("心情："));
    }

    #[test]
    fn an_empty_archive_says_so_instead_of_printing_an_empty_page() {
        let text = notes_markdown(&Data::empty("dev-a"), "2026-10-28 14:05");
        assert!(text.contains("还没有记录。"));
        assert!(!text.contains("共 0 天"));
    }

    #[test]
    fn the_weekly_review_turns_fact_lines_into_a_bullet_list() {
        let review = Review {
            week_start: "2026-10-19".to_string(),
            week_end: "2026-10-25".to_string(),
            lines: vec![
                "阅读：完成 2/3 晚".to_string(),
                "记录：5 条，最常写到 #工作（3 次）".to_string(),
            ],
        };
        let text = review_markdown(&review);
        assert!(text.starts_with("# 栖迟 · 上周回顾\n\n区间：2026-10-19 – 2026-10-25\n"));
        assert!(text.contains("- 阅读：完成 2/3 晚"));
        assert!(text.contains("- 记录：5 条，最常写到 #工作（3 次）"));
        assert!(text.contains("不做评价"));
    }

    #[test]
    fn an_empty_review_keeps_the_rule_sentence_and_drops_a_fake_range() {
        let review = Review {
            week_start: String::new(),
            week_end: String::new(),
            lines: vec!["还没有可以回顾的一周。".to_string()],
        };
        let text = review_markdown(&review);
        assert!(!text.contains("区间："));
        assert!(text.contains("- 还没有可以回顾的一周。"));
    }
}
