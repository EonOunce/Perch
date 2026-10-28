//! 同步合并引擎（DESIGN §4.5）。纯函数：不读时钟、不碰网络、不 `use tauri`。
//!
//! 三条前提，Android 端复用同一口径时必须照搬：
//!
//! 1. **并集按 id，缺失不等于删除**。删除只写 `tombstones`，实体行不硬删、读取侧过滤（§3.3）。
//!    合并绝不会因为「远端这一条不在」就把它抹掉——那会把另一台设备的正常状态当成删除。
//! 2. **两端各算各的，结果必须一模一样**。`merge(a, b, base)` 与 `merge(b, a, base)` 要产出同一份内容，
//!    否则两台设备永远合不到一起、每次都「有变化」反复上传。所以胜负只由被比较的两条记录自身决定
//!    （时间戳 → 内容编码），不看「谁是本地、谁是远端」。合并也因此不造时钟：冲突副本沿用败者自己的时间戳。
//! 3. **正文不静默丢**。同一 id 的记录双方都改过且改得不一样，一份保留原 id，另一份另存成冲突副本
//!    （§4.5 的偏离说明：谁留原 id 由内容决定，不是固定的「远端优先」）。
//!
//! 基线（`sync-base.json`）只用来判断「这一侧动过正文没有」，本身不参与并集。
//!
//! ## 内容编码（跨语言约定）
//!
//! 时间戳相同而内容不同时用字典序决胜负。编码规则：字段之间用 U+001F、列表元素之间用 U+001E、
//! 集合之间用 U+001D；值里的反斜杠与这三个控制字符先转义成 `\\` `\f` `\e` `\g`；
//! 可空字段有值时加 `1` 前缀、`null` 写成空串；布尔用 `1` / `0`；枚举用 JSON 里那几个字符串
//! （`sameDay` / `weekly` / `done` …）；字段顺序就是下面每个 `*_code` 里写的顺序；
//! 集合先按编码排序再拼，所以存储顺序不影响摘要。时间戳一律先解析成毫秒再比，
//! 读不出来排最后（与 [`crate::habits::effective_events`] 同口径）——静默丢数据比丢顺序难查。

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::date_boundary::parse_instant;
use crate::model::{
    CheckinEvent, CheckinStatus, ConfirmationTiming, DailyMood, Data, EntityType, GoalEntry, Habit,
    Marker, Note, TargetMode, Tombstone, SCHEMA_VERSION,
};

const FIELD: &str = "\u{1f}";
const ITEM: &str = "\u{1e}";
const GROUP: &str = "\u{1d}";

/// 冲突副本 id 的分隔符，接在原 id 后面，一眼看得出是哪条的副本。
const COPY: char = '#';

/// 两份快照合并成一份。`base` 是本机上次合并后的远端快照；`None` 表示首次同步或基线丢了。
pub fn merge(local: &Data, remote: &Data, base: Option<&Data>) -> Data {
    let mut graves: BTreeMap<String, Tombstone> = BTreeMap::new();
    for t in local.tombstones.iter().chain(&remote.tombstones) {
        keep_grave(&mut graves, t);
    }
    Data {
        // 写出去的永远是本程序认得的版本；更高的版本由 [`unknown_schema`] 挡住上传。
        schema_version: SCHEMA_VERSION,
        // 时间戳由宿主在落盘/上传前盖（`storage::Store::save`）。
        generated_at: None,
        // 这份快照由本机写出去，设备号是它自己那一个。
        device_id: local.device_id.clone(),
        habits: merge_habits(local, remote),
        markers: merge_markers(local, remote),
        checkin_events: merge_events(local, remote),
        notes: merge_notes(local, remote, base),
        daily_moods: merge_moods(local, remote, &mut graves),
        tombstones: graves.into_values().collect(),
    }
}

/// 本程序看不懂的更高 `schemaVersion`（DESIGN §4.7）。未知字段在反序列化时已经丢掉，
/// 拿这样一份数据上传等于把远端降级，所以只能读、只能提示升级。
pub fn unknown_schema(data: &Data) -> bool {
    data.schema_version > SCHEMA_VERSION
}

/// 内容摘要：只含业务内容，不含 `generatedAt` 与 `deviceId`。
/// §4.4 用「合并后的摘要与远端一致、且本地没新变更」决定要不要 PUT；两端算出的字符串必须相等。
pub fn fingerprint(data: &Data) -> String {
    let mut out = String::from("perch-merge-1");
    section(
        &mut out,
        "habit",
        data.habits.iter().map(habit_code).collect(),
    );
    section(
        &mut out,
        "marker",
        data.markers.iter().map(marker_code).collect(),
    );
    section(
        &mut out,
        "checkinEvent",
        data.checkin_events.iter().map(event_code).collect(),
    );
    section(&mut out, "note", data.notes.iter().map(note_code).collect());
    section(
        &mut out,
        "dailyMood",
        data.daily_moods.iter().map(mood_code).collect(),
    );
    section(
        &mut out,
        "tombstone",
        data.tombstones.iter().map(tombstone_code).collect(),
    );
    out
}

fn section(out: &mut String, name: &str, mut codes: Vec<String>) {
    codes.sort();
    out.push_str(name);
    out.push_str(FIELD);
    out.push_str(&codes.join(ITEM));
    out.push_str(GROUP);
}

// ── habit ────────────────────────────────────────────────────────────────

fn merge_habits(local: &Data, remote: &Data) -> Vec<Habit> {
    pair_by_id(&local.habits, &remote.habits, |h| &h.id, habit_rank)
        .into_values()
        .map(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => {
                // 习惯级 LWW 的赢家决定同生效日那条目标取谁的，输家只补赢家没有的生效日。
                let (win, lose) = rank_order(a, b, habit_rank);
                let mut h = (*win).clone();
                h.goal_history = merge_goal_history(win, lose);
                h
            }
            (Some(h), None) | (None, Some(h)) => (*h).clone(),
            (None, None) => unreachable!("pair_by_id 不会给出两侧都空的键"),
        })
        .collect()
}

/// `goalHistory` 按 `effectiveFrom` 取并集。条目除生效日以外没有时间戳，同一生效日谁新只能猜，
/// 所以按「习惯级 LWW 赢家那侧」定，不再发明第二个口径。
fn merge_goal_history(win: &Habit, lose: &Habit) -> Vec<GoalEntry> {
    let mut by_day: BTreeMap<&str, &GoalEntry> = BTreeMap::new();
    for e in &lose.goal_history {
        by_day.entry(&e.effective_from).or_insert(e);
    }
    for e in &win.goal_history {
        by_day.insert(&e.effective_from, e);
    }
    by_day.into_values().cloned().collect()
}

// ── marker / checkinEvent ────────────────────────────────────────────────

fn merge_markers(local: &Data, remote: &Data) -> Vec<Marker> {
    union_lww(&local.markers, &remote.markers, |m| &m.id, marker_rank)
}

/// 打卡事件只并原始记录，「同晚哪条有效」仍归 [`crate::habits::effective_events`] 判。
/// 这里再判一次就出现两份口径，违反 AGENTS.md「口径只有一处来源」。
fn merge_events(local: &Data, remote: &Data) -> Vec<CheckinEvent> {
    union_lww(
        &local.checkin_events,
        &remote.checkin_events,
        |e| &e.id,
        event_rank,
    )
}

fn union_lww<'a, T, K>(
    local: &'a [T],
    remote: &'a [T],
    id: impl Fn(&'a T) -> &'a K,
    rank: impl Fn(&'a T) -> (i64, String),
) -> Vec<T>
where
    T: Clone,
    K: Ord + 'a,
{
    pair_by_id(local, remote, id, &rank)
        .into_values()
        .map(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => (*pick(a, b, &rank)).clone(),
            (Some(a), None) | (None, Some(a)) => (*a).clone(),
            (None, None) => unreachable!("pair_by_id 不会给出两侧都空的键"),
        })
        .collect()
}

// ── note ─────────────────────────────────────────────────────────────────

fn merge_notes(local: &Data, remote: &Data, base: Option<&Data>) -> Vec<Note> {
    // 基线里同一 id 出现两次时取后一条：那是坏数据，这里只需要它回答「正文动过没有」。
    let baseline: HashMap<&str, &Note> = base
        .map(|b| b.notes.iter().map(|n| (n.id.as_str(), n)).collect())
        .unwrap_or_default();
    let pairs = pair_by_id(&local.notes, &remote.notes, |n| &n.id, note_rank);

    let mut taken: HashSet<String> = pairs.keys().map(|k| (*k).to_string()).collect();
    let mut out: Vec<Note> = Vec::new();
    // 冲突副本留到第二轮再生成：`out` 得先收齐两侧所有活下来的记录，才认得出「同样的正文已经在别的 id 上了」。
    let mut copies: Vec<(String, Note)> = Vec::new();
    for (id, (a, b)) in pairs {
        let (a, b) = match (a, b) {
            // 两侧一模一样、或只有一侧有：没有并发，直接收。
            (Some(a), Some(b)) if note_code(a) == note_code(b) => {
                out.push((*a).clone());
                continue;
            }
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) | (None, Some(a)) => {
                out.push((*a).clone());
                continue;
            }
            (None, None) => continue,
        };
        // 「改过」只认正文与标签（§4.5）：改了排序之类的附属字段不该惊动副本。
        let changed = |side: &Note| match baseline.get(id.as_str()) {
            Some(before) => !same_prose(side, before),
            // 没有基线，或基线里本来没这条（这一侧新建的）：保守当作改过。
            None => true,
        };
        if !(changed(a) && changed(b)) {
            let win = if changed(a) {
                a
            } else if changed(b) {
                b
            } else {
                pick(a, b, note_rank)
            };
            out.push((*win).clone());
            continue;
        }
        let (win, lose) = rank_order(a, b, note_rank);
        out.push((*win).clone());
        copies.push(((*id).to_string(), (*lose).clone()));
    }
    for (original, mut lose) in copies {
        // 同一天的同样正文已经在别处了，就不再造第二份：正文没丢，副本只是噪音。
        // 少了这一步，「上一轮没传上去」会一轮积一份副本（同步状态机的重试会踩到）。
        let key = prose_key(&lose);
        if out.iter().any(|n| prose_key(n) == key) {
            continue;
        }
        lose.conflict_of = Some(original.clone());
        lose.id = free_copy_id(&mut taken, &original, &lose.updated_at);
        out.push(lose);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 「同样的正文」= 同一天的同样文字加同样标签。日期也要算进去：
/// 换个日子写过同样的话是另一条记录，把它当重复丢掉才是真丢内容。
fn prose_key(note: &Note) -> (&str, &[String], &str) {
    (
        note.text.as_str(),
        note.tags.as_slice(),
        note.local_date.as_str(),
    )
}

fn same_prose(a: &Note, b: &Note) -> bool {
    a.text == b.text && a.tags == b.tags
}

/// 副本 id = `原id#败者时间戳的字母数字部分`，被占了再补 `-2`、`-3`。
/// 只用败者自身的字段生成，两台设备才会算出同一个 id；不引哈希函数，免得两端各实现一种。
fn free_copy_id(taken: &mut HashSet<String>, original: &str, stamp: &str) -> String {
    let slug: String = stamp
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let slug = if slug.is_empty() { "none" } else { &slug };
    let mut candidate = format!("{original}{COPY}{slug}");
    let mut n = 2;
    while !taken.insert(candidate.clone()) {
        candidate = format!("{original}{COPY}{slug}-{n}");
        n += 1;
    }
    candidate
}

// ── dailyMood ────────────────────────────────────────────────────────────

fn merge_moods(
    local: &Data,
    remote: &Data,
    graves: &mut BTreeMap<String, Tombstone>,
) -> Vec<DailyMood> {
    let mut by_day: BTreeMap<&str, Vec<&DailyMood>> = BTreeMap::new();
    for m in local.daily_moods.iter().chain(&remote.daily_moods) {
        by_day.entry(m.local_date.as_str()).or_default().push(m);
    }
    let mut out: Vec<DailyMood> = Vec::new();
    for (_, rows) in by_day {
        let win = rows
            .iter()
            .copied()
            .reduce(|a, b| pick(a, b, mood_rank))
            .expect("分组里至少有一条");
        // 这一天里任何一条被删过，活下来的那条也算删。心情按 `localDate` 一天一条，
        // 两台设备可能各自用了不同 id：只认 id 的墓碑会被另一侧的新 id 绕过，删掉又复活。
        for m in rows.iter() {
            let already = graves
                .get(&grave_key(EntityType::DailyMood, &m.id))
                .cloned();
            if let Some(g) = already {
                keep_grave(
                    graves,
                    &Tombstone {
                        entity_type: EntityType::DailyMood,
                        entity_id: win.id.clone(),
                        deleted_at: g.deleted_at,
                        deleted_by: g.deleted_by,
                    },
                );
            }
        }
        out.push((*win).clone());
    }
    out.sort_by(|a, b| {
        a.local_date
            .cmp(&b.local_date)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

// ── tombstone ────────────────────────────────────────────────────────────

/// 同一 `(entityType, entityId)` 只留一份：删除时间最早的那条；同一瞬间按 `deletedBy` 小者。
/// 留哪份必须与来路无关，否则两台设备的摘要永远对不上。V1 不清理（§4.5）。
fn keep_grave(graves: &mut BTreeMap<String, Tombstone>, t: &Tombstone) {
    let key = grave_key(t.entity_type, &t.entity_id);
    let keep_this = match graves.get(&key) {
        Some(cur) => {
            (ms(&t.deleted_at), t.deleted_by.as_str())
                < (ms(&cur.deleted_at), cur.deleted_by.as_str())
        }
        None => true,
    };
    if keep_this {
        graves.insert(key, t.clone());
    }
}

fn grave_key(entity_type: EntityType, id: &str) -> String {
    format!("{}{FIELD}{id}", entity_type_name(entity_type))
}

// ── 公共小工具 ────────────────────────────────────────────────────────────

/// 按 id 把两份快照的同一集合对齐。某一侧内部重复（坏数据）就地按 LWW 收敛成一条，
/// 不让它冒充「另一侧的并发版本」。
fn pair_by_id<'a, T, K>(
    local: &'a [T],
    remote: &'a [T],
    id: impl Fn(&'a T) -> &'a K,
    rank: impl Fn(&'a T) -> (i64, String),
) -> BTreeMap<&'a K, (Option<&'a T>, Option<&'a T>)>
where
    K: Ord,
{
    let mut out: BTreeMap<&'a K, (Option<&'a T>, Option<&'a T>)> = BTreeMap::new();
    for t in local {
        let e = out.entry(id(t)).or_insert((None, None));
        e.0 = Some(match e.0 {
            Some(seen) => pick(seen, t, &rank),
            None => t,
        });
    }
    for t in remote {
        let e = out.entry(id(t)).or_insert((None, None));
        e.1 = Some(match e.1 {
            Some(seen) => pick(seen, t, &rank),
            None => t,
        });
    }
    out
}

/// 时间戳新的赢；完全相同（同一瞬间、内容也不同）时比内容编码，两端才会挑出同一条。
fn pick<'a, T>(a: &'a T, b: &'a T, rank: impl Fn(&'a T) -> (i64, String)) -> &'a T {
    if rank(b) > rank(a) {
        b
    } else {
        a
    }
}

fn rank_order<'a, T>(a: &'a T, b: &'a T, rank: impl Fn(&'a T) -> (i64, String)) -> (&'a T, &'a T) {
    if rank(b) > rank(a) {
        (b, a)
    } else {
        (a, b)
    }
}

fn ms(stamp: &str) -> i64 {
    parse_instant(stamp).unwrap_or(i64::MIN)
}

fn habit_rank(h: &Habit) -> (i64, String) {
    (ms(&h.updated_at), habit_code(h))
}

fn marker_rank(m: &Marker) -> (i64, String) {
    (ms(&m.updated_at), marker_code(m))
}

/// 事件没有 `updatedAt`（原始事实只追加、更正另起一条并写 `supersedesId`），所以按 `createdAt`。
fn event_rank(e: &CheckinEvent) -> (i64, String) {
    (ms(&e.created_at), event_code(e))
}

fn note_rank(n: &Note) -> (i64, String) {
    (ms(&n.updated_at), note_code(n))
}

fn mood_rank(m: &DailyMood) -> (i64, String) {
    (ms(&m.updated_at), mood_code(m))
}

// ── 编码 ─────────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\u{1f}' => out.push_str("\\f"),
            '\u{1e}' => out.push_str("\\e"),
            '\u{1d}' => out.push_str("\\g"),
            _ => out.push(c),
        }
    }
    out
}

fn opt_str(v: Option<&str>) -> String {
    v.map(|s| format!("1{s}")).unwrap_or_default()
}

fn opt_i32(v: Option<i32>) -> String {
    v.map(|n| format!("1n{n}")).unwrap_or_default()
}

fn flag(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

fn items(v: &[String]) -> String {
    v.iter().map(|s| esc(s)).collect::<Vec<_>>().join(ITEM)
}

fn entity_type_name(t: EntityType) -> &'static str {
    match t {
        EntityType::Habit => "habit",
        EntityType::Marker => "marker",
        EntityType::CheckinEvent => "checkinEvent",
        EntityType::Note => "note",
        EntityType::DailyMood => "dailyMood",
    }
}

fn timing_name(t: ConfirmationTiming) -> &'static str {
    match t {
        ConfirmationTiming::SameDay => "sameDay",
        ConfirmationTiming::NextDay => "nextDay",
    }
}

fn mode_name(m: TargetMode) -> &'static str {
    match m {
        TargetMode::RecordOnly => "recordOnly",
        TargetMode::Weekly => "weekly",
        TargetMode::Nightly => "nightly",
        TargetMode::Custom => "custom",
    }
}

fn status_name(s: CheckinStatus) -> &'static str {
    match s {
        CheckinStatus::Done => "done",
        CheckinStatus::NotDone => "notDone",
        CheckinStatus::Void => "void",
    }
}

fn habit_code(h: &Habit) -> String {
    let goals: Vec<String> = h.goal_history.iter().map(goal_code).collect();
    [
        esc(&h.id),
        esc(&h.name),
        esc(&h.icon),
        esc(&h.color),
        esc(&h.operational_definition),
        esc(&h.unit),
        h.target_count.to_string(),
        timing_name(h.confirmation_timing).to_string(),
        mode_name(h.target_mode).to_string(),
        opt_i32(h.weekly_target),
        esc(&h.if_then),
        opt_str(h.reminder_time.as_deref()),
        h.sort_order.to_string(),
        opt_str(h.archived_at.as_deref()),
        flag(h.stats_visible).to_string(),
        esc(&h.created_at),
        esc(&h.updated_at),
        items(&goals),
    ]
    .join(FIELD)
}

fn goal_code(g: &GoalEntry) -> String {
    [
        esc(&g.effective_from),
        mode_name(g.target_mode).to_string(),
        opt_i32(g.weekly_target),
    ]
    .join(FIELD)
}

fn marker_code(m: &Marker) -> String {
    [
        esc(&m.id),
        esc(&m.habit_id),
        esc(&m.local_date),
        esc(&m.label),
        esc(&m.created_at),
        esc(&m.updated_at),
    ]
    .join(FIELD)
}

fn event_code(ev: &CheckinEvent) -> String {
    [
        esc(&ev.id),
        esc(&ev.habit_id),
        esc(&ev.local_date),
        status_name(ev.status).to_string(),
        opt_str(ev.confirmed_at.as_deref()),
        esc(&ev.source),
        opt_str(ev.supersedes_id.as_deref()),
        esc(&ev.recorded_by),
        esc(&ev.note),
        esc(&ev.created_at),
    ]
    .join(FIELD)
}

fn note_code(n: &Note) -> String {
    [
        esc(&n.id),
        esc(&n.created_at),
        esc(&n.local_date),
        esc(&n.text),
        items(&n.tags),
        esc(&n.updated_at),
        opt_str(n.conflict_of.as_deref()),
    ]
    .join(FIELD)
}

fn mood_code(m: &DailyMood) -> String {
    [
        esc(&m.id),
        esc(&m.local_date),
        m.score.to_string(),
        esc(&m.updated_at),
    ]
    .join(FIELD)
}

fn tombstone_code(t: &Tombstone) -> String {
    [
        entity_type_name(t.entity_type).to_string(),
        esc(&t.entity_id),
        esc(&t.deleted_at),
        esc(&t.deleted_by),
    ]
    .join(FIELD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::habits;

    const DAY: &str = "2026-10-21";

    fn habit(id: &str, name: &str, updated: &str) -> Habit {
        Habit {
            id: id.to_string(),
            name: name.to_string(),
            icon: String::new(),
            color: String::new(),
            operational_definition: String::new(),
            unit: "次".to_string(),
            target_count: 1,
            confirmation_timing: ConfirmationTiming::SameDay,
            target_mode: TargetMode::Weekly,
            weekly_target: Some(4),
            if_then: String::new(),
            reminder_time: None,
            sort_order: 0,
            archived_at: None,
            stats_visible: true,
            goal_history: Vec::new(),
            created_at: "2026-10-01T09:00:00.000+08:00".to_string(),
            updated_at: updated.to_string(),
        }
    }

    fn goal(from: &str, weekly: i32) -> GoalEntry {
        GoalEntry {
            effective_from: from.to_string(),
            target_mode: TargetMode::Weekly,
            weekly_target: Some(weekly),
        }
    }

    fn note(id: &str, text: &str, updated: &str) -> Note {
        Note {
            id: id.to_string(),
            created_at: "2026-10-20T09:00:00.000+08:00".to_string(),
            local_date: DAY.to_string(),
            text: text.to_string(),
            tags: Vec::new(),
            updated_at: updated.to_string(),
            conflict_of: None,
        }
    }

    fn marker(id: &str, label: &str, updated: &str) -> Marker {
        Marker {
            id: id.to_string(),
            habit_id: "h1".to_string(),
            local_date: DAY.to_string(),
            label: label.to_string(),
            created_at: "2026-10-20T09:00:00.000+08:00".to_string(),
            updated_at: updated.to_string(),
        }
    }

    fn event(id: &str, status: CheckinStatus, created: &str) -> CheckinEvent {
        CheckinEvent {
            id: id.to_string(),
            habit_id: "h1".to_string(),
            local_date: DAY.to_string(),
            status,
            confirmed_at: None,
            source: "manual".to_string(),
            supersedes_id: None,
            recorded_by: "pc".to_string(),
            note: String::new(),
            created_at: created.to_string(),
        }
    }

    fn mood(id: &str, day: &str, score: i32, updated: &str) -> DailyMood {
        DailyMood {
            id: id.to_string(),
            local_date: day.to_string(),
            score,
            updated_at: updated.to_string(),
        }
    }

    fn grave(ty: EntityType, id: &str, deleted_at: &str, by: &str) -> Tombstone {
        Tombstone {
            entity_type: ty,
            entity_id: id.to_string(),
            deleted_at: deleted_at.to_string(),
            deleted_by: by.to_string(),
        }
    }

    fn snapshot(device: &str, habits: Vec<Habit>, notes: Vec<Note>) -> Data {
        let mut d = Data::empty(device);
        d.habits = habits;
        d.notes = notes;
        d
    }

    fn pair(device: &str) -> (Data, Data) {
        (Data::empty(device), Data::empty(device))
    }

    #[test]
    fn presence_on_one_side_is_kept_even_though_the_other_side_lacks_it() {
        let (mut a, mut b) = pair("pc");
        a.habits
            .push(habit("h1", "喝水", "2026-10-20T09:00:00.000+08:00"));
        b.habits
            .push(habit("h2", "阅读", "2026-10-20T09:00:00.000+08:00"));
        let merged = merge(&a, &b, None);
        assert_eq!(
            merged
                .habits
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            vec!["h1", "h2"],
            "远端没有这条不等于它被删了"
        );
    }

    #[test]
    fn tombstone_survives_and_still_filters_the_habit_out() {
        let (mut a, mut b) = pair("pc");
        let h = habit("h1", "喝水", "2026-10-20T09:00:00.000+08:00");
        a.habits.push(h.clone());
        a.tombstones.push(grave(
            EntityType::Habit,
            "h1",
            "2026-10-21T09:00:00.000+08:00",
            "pc",
        ));
        b.habits
            .push(habit("h1", "喝水", "2026-10-22T09:00:00.000+08:00"));
        let merged = merge(&a, &b, None);
        assert_eq!(merged.tombstones.len(), 1, "墓碑并集后要留下");
        assert_eq!(merged.habits.len(), 1, "实体行不硬删，读取侧过滤");
        assert!(
            habits::is_removed(&merged, &merged.habits[0]),
            "删了又复活是绝对不该出现的"
        );
        assert!(habits::today_list(&merged).is_empty());
    }

    #[test]
    fn later_update_wins_the_habit_row() {
        let (mut a, mut b) = pair("pc");
        a.habits
            .push(habit("h1", "本机改的名", "2026-10-20T09:00:00.000+08:00"));
        b.habits
            .push(habit("h1", "远端改的名", "2026-10-21T09:00:00.000+08:00"));
        let merged = merge(&a, &b, None);
        assert_eq!(merged.habits[0].name, "远端改的名");
        // 反过来喂同一对，赢家还是同一条。
        let flipped = merge(&b, &a, None);
        assert_eq!(flipped.habits[0].name, "远端改的名");
    }

    #[test]
    fn same_instant_breaks_ties_by_content_code() {
        let stamp = "2026-10-21T09:00:00.000+08:00";
        let (mut a, mut b) = pair("pc");
        a.habits.push(habit("h1", "aaa", stamp));
        b.habits.push(habit("h1", "bbb", stamp));
        assert_eq!(merge(&a, &b, None).habits[0].name, "bbb");
        assert_eq!(merge(&b, &a, None).habits[0].name, "bbb");
    }

    #[test]
    fn unparseable_timestamp_is_not_treated_as_the_oldest_winning_row() {
        let (mut a, mut b) = pair("pc");
        a.habits.push(habit("h1", "坏时间戳", "not-a-stamp"));
        b.habits
            .push(habit("h1", "好时间戳", "2026-10-21T09:00:00.000+08:00"));
        assert_eq!(merge(&a, &b, None).habits[0].name, "好时间戳");
    }

    #[test]
    fn goal_history_unions_with_the_winning_side_keeping_its_day() {
        let (mut a, mut b) = pair("pc");
        let mut older = habit("h1", "喝水", "2026-10-20T09:00:00.000+08:00");
        older.goal_history = vec![goal("2026-10-05", 4), goal("2026-10-12", 5)];
        let mut newer = habit("h1", "喝水", "2026-10-21T09:00:00.000+08:00");
        newer.goal_history = vec![goal("2026-10-05", 3), goal("2026-10-19", 6)];
        a.habits.push(newer);
        b.habits.push(older);
        let merged = merge(&a, &b, None);
        let days: Vec<&str> = merged.habits[0]
            .goal_history
            .iter()
            .map(|g| g.effective_from.as_str())
            .collect();
        assert_eq!(days, vec!["2026-10-05", "2026-10-12", "2026-10-19"]);
        assert_eq!(
            merged.habits[0].goal_history[0].weekly_target,
            Some(3),
            "同一生效日归习惯级赢家那一侧"
        );
        assert_eq!(
            fingerprint(&merge(&b, &a, None)),
            fingerprint(&merged),
            "goalHistory 也得对称"
        );
    }

    #[test]
    fn markers_union_by_id_and_identical_duplicates_collapse() {
        let (mut a, mut b) = pair("pc");
        a.markers
            .push(marker("m1", "换了做法", "2026-10-20T09:00:00.000+08:00"));
        a.markers
            .push(marker("m1", "换了做法", "2026-10-20T09:00:00.000+08:00"));
        b.markers
            .push(marker("m2", "又调整一次", "2026-10-21T09:00:00.000+08:00"));
        let merged = merge(&a, &b, None);
        assert_eq!(
            merged
                .markers
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "m2"],
            "重复即幂等丢弃"
        );
    }

    #[test]
    fn checkin_events_are_unioned_raw_and_resolution_stays_at_read_time() {
        let (mut a, mut b) = pair("pc");
        a.habits
            .push(habit("h1", "喝水", "2026-10-20T09:00:00.000+08:00"));
        b.habits
            .push(habit("h1", "喝水", "2026-10-20T09:00:00.000+08:00"));
        a.checkin_events.push(event(
            "e1",
            CheckinStatus::Done,
            "2026-10-21T20:00:00.000+08:00",
        ));
        b.checkin_events.push(event(
            "e2",
            CheckinStatus::NotDone,
            "2026-10-21T21:00:00.000+08:00",
        ));
        let merged = merge(&a, &b, None);
        assert_eq!(
            merged.checkin_events.len(),
            2,
            "原始事件永远保留，合并不裁决"
        );
        let effective = habits::effective_events(&merged, "h1");
        assert_eq!(effective.len(), 1, "同一晚仍只有一个有效状态");
        assert_eq!(effective.get(DAY).unwrap().id, "e2");
    }

    #[test]
    fn concurrent_note_edits_keep_both_texts() {
        let base = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let local = snapshot(
            "pc",
            vec![],
            vec![note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00")],
        );
        let remote = snapshot(
            "phone",
            vec![],
            vec![note("n1", "手机写的", "2026-10-21T09:00:00.000+08:00")],
        );
        let merged = merge(&local, &remote, Some(&base));
        assert_eq!(merged.notes.len(), 2, "两份正文都不许丢");
        assert_eq!(merged.notes[0].id, "n1");
        assert_eq!(merged.notes[0].text, "本机写的", "较新的一份保留原 id");
        assert_eq!(merged.notes[1].conflict_of.as_deref(), Some("n1"));
        assert_eq!(merged.notes[1].text, "手机写的");
        assert!(
            merged.notes[1].id.starts_with("n1#"),
            "副本 id 要看得出是哪条的副本，实际 {:?}",
            merged.notes[1].id
        );
        assert_eq!(
            fingerprint(&merge(&remote, &local, Some(&base))),
            fingerprint(&merged),
            "谁留原 id 不能看谁是本地，否则两端合不到一起"
        );
    }

    #[test]
    fn a_loser_whose_text_already_lives_elsewhere_gets_no_second_copy() {
        // 重试路径：上一轮合并算出的副本已经传上去了，这一侧还没成功写入。
        // 再合一次时败者的正文已经在别的 id 上活着，不该再积出一份一模一样的。
        let base = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let mut copy = note("n1", "手机写的", "2026-10-21T09:00:00.000+08:00");
        copy.conflict_of = Some("n1".to_string());
        copy.id = "n1#20261021T0900000000800".to_string();
        let phone = snapshot(
            "phone",
            vec![],
            vec![note("n1", "手机写的", "2026-10-21T09:00:00.000+08:00")],
        );
        let pc = snapshot(
            "pc",
            vec![],
            vec![
                note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00"),
                copy.clone(),
            ],
        );
        let merged = merge(&phone, &pc, Some(&base));
        assert_eq!(merged.notes.len(), 2, "两份正文，不该有第三份");
        assert_eq!(merged.notes[0].id, "n1");
        assert_eq!(merged.notes[1].id, copy.id, "沿用已经在那儿的副本 id");
        assert_eq!(
            merged.notes.iter().filter(|n| n.text == "手机写的").count(),
            1,
            "同样的正文只留一份"
        );
        assert_eq!(
            fingerprint(&merge(&pc, &phone, Some(&base))),
            fingerprint(&merged)
        );
    }

    #[test]
    fn a_copy_is_still_made_when_the_same_text_belongs_to_another_day() {
        // 换个日子写过同样的话是另一条记录，不能当重复丢掉——那才是真丢内容。
        let base = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let mut twin = note("n9", "手机写的", "2026-10-21T09:00:00.000+08:00");
        twin.local_date = "2026-10-19".to_string();
        let phone = snapshot(
            "phone",
            vec![],
            vec![
                note("n1", "手机写的", "2026-10-21T09:00:00.000+08:00"),
                twin,
            ],
        );
        let pc = snapshot(
            "pc",
            vec![],
            vec![note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00")],
        );
        let merged = merge(&phone, &pc, Some(&base));
        assert_eq!(merged.notes.len(), 3, "败者要留下，副本 id 现算");
        assert_eq!(
            merged
                .notes
                .iter()
                .filter(|n| n.conflict_of.as_deref() == Some("n1"))
                .count(),
            1
        );
    }

    #[test]
    fn only_one_side_changed_the_text_so_no_copy_is_made() {
        let base = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let local = snapshot(
            "pc",
            vec![],
            vec![note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00")],
        );
        let remote = snapshot(
            "phone",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let merged = merge(&local, &remote, Some(&base));
        assert_eq!(merged.notes.len(), 1);
        assert_eq!(merged.notes[0].text, "本机写的");
    }

    #[test]
    fn a_side_that_only_touched_other_fields_is_not_a_conflict() {
        let base = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let mut local = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-22T09:00:00.000+08:00")],
        );
        local.notes[0].local_date = "2026-10-19".to_string();
        let merged = merge(&local, &base, Some(&base));
        assert_eq!(merged.notes.len(), 1, "只改附属字段不该长出副本");
        assert_eq!(
            merged.notes[0].local_date, "2026-10-19",
            "改过的一侧直接胜出"
        );
    }

    #[test]
    fn without_a_baseline_any_difference_becomes_a_copy() {
        let local = snapshot(
            "pc",
            vec![],
            vec![note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00")],
        );
        let remote = snapshot(
            "phone",
            vec![],
            vec![note("n1", "手机写的", "2026-10-21T09:00:00.000+08:00")],
        );
        assert_eq!(merge(&local, &remote, None).notes.len(), 2);
    }

    #[test]
    fn copy_ids_do_not_collide() {
        let base = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let local = snapshot(
            "pc",
            vec![],
            vec![note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00")],
        );
        let loser_stamp = "2026-10-21T09:00:00.000+08:00";
        // 本该生成的副本 id 已经被一条内容不同的记录占掉。
        let clash = format!(
            "n1#{}",
            loser_stamp
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        );
        let mut remote = snapshot("phone", vec![], vec![note("n1", "手机写的", loser_stamp)]);
        remote
            .notes
            .push(note(&clash, "恰好占了这个名字", loser_stamp));
        let merged = merge(&local, &remote, Some(&base));
        let ids: Vec<&str> = merged.notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids.len(), merged.notes.len(), "副本 id 不许撞车：{ids:?}");
        assert!(
            ids.iter().any(|i| i.ends_with("-2")),
            "被撞后应补计数后缀，实际 {ids:?}"
        );
    }

    #[test]
    fn moods_keep_one_row_per_day_by_natural_key() {
        let (mut a, mut b) = pair("pc");
        a.daily_moods
            .push(mood("m1", DAY, 40, "2026-10-21T09:00:00.000+08:00"));
        b.daily_moods
            .push(mood("m2", DAY, 70, "2026-10-22T09:00:00.000+08:00"));
        b.daily_moods.push(mood(
            "m3",
            "2026-10-20",
            55,
            "2026-10-20T09:00:00.000+08:00",
        ));
        let merged = merge(&a, &b, None);
        assert_eq!(merged.daily_moods.len(), 2, "一天最多一条");
        assert_eq!(merged.daily_moods[0].local_date, "2026-10-20");
        assert_eq!(merged.daily_moods[1].score, 70, "后写胜出");
    }

    #[test]
    fn a_mood_tombstone_on_the_other_id_still_deletes_the_day() {
        let (mut a, mut b) = pair("pc");
        a.daily_moods
            .push(mood("m1", DAY, 40, "2026-10-21T09:00:00.000+08:00"));
        a.tombstones.push(grave(
            EntityType::DailyMood,
            "m1",
            "2026-10-23T09:00:00.000+08:00",
            "pc",
        ));
        b.daily_moods
            .push(mood("m2", DAY, 70, "2026-10-22T09:00:00.000+08:00"));
        let merged = merge(&a, &b, None);
        assert_eq!(merged.daily_moods.len(), 1);
        assert_eq!(merged.daily_moods[0].id, "m2");
        assert!(
            habits::is_tombstoned(&merged, EntityType::DailyMood, "m2"),
            "自然键的删除要落在自然键上，否则换 id 重写同一天会让删掉的心情复活"
        );
        assert_eq!(fingerprint(&merge(&b, &a, None)), fingerprint(&merged));
    }

    #[test]
    fn tombstones_keep_the_earliest_deletion() {
        let (mut a, mut b) = pair("pc");
        a.tombstones.push(grave(
            EntityType::Note,
            "n1",
            "2026-10-23T09:00:00.000+08:00",
            "zz",
        ));
        b.tombstones.push(grave(
            EntityType::Note,
            "n1",
            "2026-10-22T09:00:00.000+08:00",
            "aa",
        ));
        let merged = merge(&a, &b, None);
        assert_eq!(merged.tombstones.len(), 1);
        assert_eq!(
            merged.tombstones[0].deleted_at,
            "2026-10-22T09:00:00.000+08:00"
        );
        // 同一瞬间按 deletedBy 小者，与来路无关。
        let same_moment = |device: &str, by: &str| {
            let mut d = Data::empty(device);
            d.tombstones.push(grave(
                EntityType::Note,
                "n1",
                "2026-10-22T09:00:00.000+08:00",
                by,
            ));
            d
        };
        let merged = merge(&same_moment("pc", "zz"), &same_moment("phone", "aa"), None);
        assert_eq!(merged.tombstones[0].deleted_by, "aa");
        assert_eq!(
            merged.tombstones[0].deleted_by,
            merge(&same_moment("phone", "aa"), &same_moment("pc", "zz"), None).tombstones[0]
                .deleted_by
        );
    }

    #[test]
    fn merging_a_snapshot_with_itself_changes_nothing() {
        let base = snapshot(
            "pc",
            vec![],
            vec![note("n1", "原文", "2026-10-20T09:00:00.000+08:00")],
        );
        let local = snapshot(
            "pc",
            vec![habit("h1", "喝水", "2026-10-21T09:00:00.000+08:00")],
            vec![note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00")],
        );
        let remote = snapshot(
            "phone",
            vec![habit("h1", "喝水", "2026-10-20T09:00:00.000+08:00")],
            vec![note("n1", "手机写的", "2026-10-21T09:00:00.000+08:00")],
        );
        let once = merge(&local, &remote, Some(&base));
        let twice = merge(&once, &once, Some(&once));
        assert_eq!(fingerprint(&twice), fingerprint(&once), "合并要幂等");
        assert_eq!(twice.notes.len(), once.notes.len());
    }

    #[test]
    fn the_merged_snapshot_carries_no_invented_clock_and_the_writing_device() {
        let a = Data::empty("pc");
        let b = Data::empty("phone");
        let merged = merge(&a, &b, None);
        assert_eq!(merged.generated_at, None, "时间戳由宿主盖，合并是纯函数");
        assert_eq!(merged.device_id, "pc", "谁写出去就署谁的设备号");
        assert_eq!(merge(&b, &a, None).device_id, "phone");
        assert_eq!(merged.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_remote_schema_version_is_flagged_but_never_written_down() {
        let (a, mut b) = pair("pc");
        b.schema_version = SCHEMA_VERSION + 1;
        b.notes
            .push(note("n1", "新版本写的", "2026-10-22T09:00:00.000+08:00"));
        assert!(unknown_schema(&b), "本程序读不懂的版本要能被认出来");
        assert!(!unknown_schema(&a));
        let merged = merge(&a, &b, None);
        assert_eq!(
            merged.schema_version, SCHEMA_VERSION,
            "不认识的版本不能冒充成自己写过的"
        );
    }

    #[test]
    fn fingerprint_ignores_metadata_and_stored_order() {
        let mut a = snapshot(
            "pc",
            vec![habit("h1", "喝水", "2026-10-21T09:00:00.000+08:00")],
            vec![
                note("n1", "第一条", "2026-10-21T09:00:00.000+08:00"),
                note("n2", "第二条", "2026-10-21T09:00:00.000+08:00"),
            ],
        );
        a.generated_at = Some("2026-10-21T10:00:00.000+08:00".to_string());
        let mut b = snapshot(
            "phone",
            vec![habit("h1", "喝水", "2026-10-21T09:00:00.000+08:00")],
            vec![
                note("n2", "第二条", "2026-10-21T09:00:00.000+08:00"),
                note("n1", "第一条", "2026-10-21T09:00:00.000+08:00"),
            ],
        );
        b.generated_at = Some("2026-10-22T10:00:00.000+08:00".to_string());
        assert_eq!(
            fingerprint(&a),
            fingerprint(&b),
            "设备号、时间戳与存储顺序都不算内容变化"
        );

        let mut changed = a.clone();
        changed.notes[0].text = "第一条改过".to_string();
        assert_ne!(fingerprint(&a), fingerprint(&changed));
    }

    #[test]
    fn escaping_keeps_field_boundaries_unambiguous() {
        let stamp = "2026-10-21T09:00:00.000+08:00";
        let odd = note("n1", &format!("a{FIELD}b"), stamp);
        let plain = note("n1", "ab", stamp);
        assert_ne!(note_code(&odd), note_code(&plain));
    }
}
