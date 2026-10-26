//! 日界与日期算术。全部是纯整数运算，不给核心拉进时区库：
//! 宿主（Windows / 手机）负责把「此刻的 UTC 偏移秒数」传进来，历史 `localDate` 一旦写下就不再重算
//! （DESIGN §3.3「历史 localDate 不随时区或 DST 改变」）。

/// 一天从 04:00 开始：凌晨 3 点的记录还算昨天。
pub const DAY_START_HOUR: u32 = 4;

pub(crate) const MS_PER_DAY: i64 = 86_400_000;

/// civil → 距 1970-01-01 的天数（Howard Hinnant 的算法，覆盖整条公历）。
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 距 1970-01-01 的天数 → 年月日。
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 解析 `YYYY-MM-DD`，并校验这个日期真实存在（`2026-02-30` 不接受）。
pub fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| s[r].bytes().all(|c| c.is_ascii_digit());
    if !digits(0..4) || !digits(5..7) || !digits(8..10) {
        return None;
    }
    let (y, m, d) = (
        s[0..4].parse::<i64>().ok()?,
        s[5..7].parse::<i64>().ok()?,
        s[8..10].parse::<i64>().ok()?,
    );
    let days = days_from_civil(y, m, d);
    // 往返一次不相等就说明不是真实存在的日子（月份越界、2 月 30 号等）。
    if civil_from_days(days) != (y, m, d) {
        return None;
    }
    Some(days)
}

pub fn format_date(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn add_days(date: &str, n: i64) -> Option<String> {
    Some(format_date(parse_date(date)? + n))
}

pub fn days_between(from: &str, to: &str) -> Option<i64> {
    Some(parse_date(to)? - parse_date(from)?)
}

/// 含首尾的日期序列。`from > to` 时返回空，不返回倒序。
pub fn date_range(from: &str, to: &str) -> Option<Vec<String>> {
    let (a, b) = (parse_date(from)?, parse_date(to)?);
    if a > b {
        return Some(Vec::new());
    }
    Some((a..=b).map(format_date).collect())
}

/// 星期几：周一 = 0 … 周日 = 6。1970-01-01 是星期四。
pub fn weekday_mon_first(date: &str) -> Option<u32> {
    Some((parse_date(date)? + 3).rem_euclid(7) as u32)
}

/// 本周周一（当天就是周一时返回当天）。新增习惯的生效日用它。
pub fn week_start(date: &str) -> Option<String> {
    add_days(date, -(weekday_mon_first(date)? as i64))
}

/// 下一个周一（严格在未来，最长 7 天后）。改已有习惯的目标用它的结果作 `effectiveFrom`。
pub fn next_week_start(date: &str) -> Option<String> {
    add_days(date, 7 - weekday_mon_first(date)? as i64)
}

/// 某个瞬间在给定 UTC 偏移下属于哪个业务日（04:00 日界）。
pub fn local_date(instant_ms: i64, offset_secs: i32, day_start_hour: u32) -> String {
    let local = instant_ms + offset_secs as i64 * 1000;
    let days = local.div_euclid(MS_PER_DAY);
    let ms_of_day = local.rem_euclid(MS_PER_DAY);
    let shift = day_start_hour as i64 * 3_600_000;
    format_date(if ms_of_day < shift { days - 1 } else { days })
}

/// 解析带时区的 ISO 8601 瞬间：`YYYY-MM-DDTHH:MM:SS`，可选 `.mmm`，尾随 `Z` 或 `±HH:MM`。
/// 解析不了返回 None，让调用方决定是丢弃还是报错，不静默当成 0。
pub fn parse_instant(iso: &str) -> Option<i64> {
    let bytes = iso.as_bytes();
    if bytes.len() < 20 || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let date = parse_date(&iso[0..10])?;
    let num = |r: std::ops::Range<usize>| iso[r].parse::<i64>().ok();
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..60).contains(&s) {
        return None;
    }
    let tail = &iso[19..];
    let (millis, zone) = if tail.starts_with('.') {
        if tail.len() < 4 {
            return None;
        }
        let frac = &tail[1..4];
        if !frac.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        (frac.parse::<i64>().ok()?, &tail[4..])
    } else {
        (0, tail)
    };
    let offset_secs = match zone {
        "Z" => 0,
        z if z.len() == 6 && (z.as_bytes()[0] == b'+' || z.as_bytes()[0] == b'-') => {
            let sign = if z.as_bytes()[0] == b'+' { 1 } else { -1 };
            let zh = z[1..3].parse::<i32>().ok()?;
            let zm = z[4..6].parse::<i32>().ok()?;
            if zh > 23 || zm > 59 {
                return None;
            }
            sign * (zh * 3600 + zm * 60)
        }
        _ => return None,
    };
    let secs = date * 86_400 + h * 3600 + mi * 60 + s - offset_secs as i64;
    Some(secs * 1000 + millis)
}

/// 写成 `YYYY-MM-DDTHH:MM:SS.mmm±HH:MM`，与 DESIGN 示例一致（毫秒固定三位）。
pub fn format_instant(instant_ms: i64, offset_secs: i32) -> String {
    let local = instant_ms + offset_secs as i64 * 1000;
    let days = local.div_euclid(MS_PER_DAY);
    let rest = local.rem_euclid(MS_PER_DAY);
    let (h, mi, s) = (rest / 3_600_000, rest / 60_000 % 60, rest / 1000 % 60);
    let millis = rest % 1000;
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let off = offset_secs.unsigned_abs();
    format!(
        "{}T{:02}:{:02}:{:02}.{:03}{}{:02}:{:02}",
        format_date(days),
        h,
        mi,
        s,
        millis,
        sign,
        off / 3600,
        off / 60 % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-26 是周一（§6.2 的「本周」按周一起算）。
    const MONDAY: &str = "2026-10-26";
    /// 上面那个周一早晨 04:00 整（+08:00）——日界切换的那一毫秒。
    const MONDAY_4AM_CST: i64 = 1_792_958_400_000;

    #[test]
    fn epoch_day_zero_is_1970_01_01() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn civil_round_trips_over_centuries() {
        // 每 997 天取一个点，能扫到闰年、世纪年和负纪元。
        for z in (-80_000..80_000i64).step_by(997) {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z, "{z} 往返不一致");
        }
    }

    #[test]
    fn only_real_dates_parse() {
        assert_eq!(parse_date(MONDAY), Some(20_752));
        assert_eq!(parse_date("2026-02-30"), None);
        assert_eq!(parse_date("2026-02-29"), None, "2026 不是闰年");
        assert_eq!(parse_date("2028-02-29"), Some(days_from_civil(2028, 2, 29)));
        assert_eq!(parse_date("2100-02-29"), None, "世纪年要能被 400 整除才闰");
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("2026-1-26"), None);
        assert_eq!(parse_date("2026-01-26 "), None);
        assert_eq!(parse_date(""), None);
    }

    #[test]
    fn day_flips_at_four() {
        assert_eq!(local_date(MONDAY_4AM_CST, 8 * 3600, DAY_START_HOUR), MONDAY);
        assert_eq!(
            local_date(MONDAY_4AM_CST - 1, 8 * 3600, DAY_START_HOUR),
            "2026-10-25",
            "凌晨 3:59 还算昨天"
        );
        // 同一瞬间在 UTC-5 属于前一天。
        assert_eq!(
            local_date(MONDAY_4AM_CST, -5 * 3600, DAY_START_HOUR),
            "2026-10-25"
        );
    }

    #[test]
    fn day_boundary_hour_is_editable() {
        // 日界改成 0 点后，凌晨那条记录就归当天了。
        assert_eq!(local_date(MONDAY_4AM_CST - 1, 8 * 3600, 0), MONDAY);
    }

    #[test]
    fn instant_parses_and_formats() {
        assert_eq!(
            parse_instant("2026-10-26T04:00:00.000+08:00"),
            Some(MONDAY_4AM_CST)
        );
        assert_eq!(parse_instant("2026-10-25T20:00:00Z"), Some(MONDAY_4AM_CST));
        assert_eq!(
            parse_instant("2026-10-26T04:00:00+08:00"),
            Some(MONDAY_4AM_CST)
        );
        assert_eq!(
            format_instant(MONDAY_4AM_CST, 8 * 3600),
            "2026-10-26T04:00:00.000+08:00"
        );
        assert_eq!(
            format_instant(MONDAY_4AM_CST, 0),
            "2026-10-25T20:00:00.000+00:00"
        );
        assert_eq!(
            format_instant(MONDAY_4AM_CST, -5 * 3600),
            "2026-10-25T15:00:00.000-05:00"
        );
    }

    #[test]
    fn instant_rejects_junk() {
        assert_eq!(parse_instant("2026-10-26"), None, "没有时刻");
        assert_eq!(
            parse_instant("2026-10-26T04:00:00"),
            None,
            "没有时区就当读不出来"
        );
        assert_eq!(parse_instant("2026-10-26T25:00:00Z"), None);
        assert_eq!(parse_instant("2026-10-26T04:60:00Z"), None);
        assert_eq!(parse_instant("2026-10-26T04:00:00+99:00"), None);
        assert_eq!(
            parse_instant("2026-10-26T04:00:00.99"),
            None,
            "小数不足三位"
        );
        assert_eq!(parse_instant("2026-10-26T04:00:00.00ZZ"), None);
        assert_eq!(parse_instant("2026-10-26 04:00:00Z"), None);
    }

    #[test]
    fn millisecond_precision_survives() {
        let ms = MONDAY_4AM_CST + 123;
        let iso = format_instant(ms, 8 * 3600);
        assert_eq!(iso, "2026-10-26T04:00:00.123+08:00");
        assert_eq!(parse_instant(&iso), Some(ms));
    }

    #[test]
    fn weekday_and_week_edges() {
        assert_eq!(weekday_mon_first(MONDAY), Some(0));
        assert_eq!(weekday_mon_first("2026-10-27"), Some(1));
        assert_eq!(weekday_mon_first("2026-11-01"), Some(6), "周日");
        assert_eq!(week_start(MONDAY).as_deref(), Some(MONDAY), "当天就是周一");
        assert_eq!(week_start("2026-11-01").as_deref(), Some(MONDAY));
        assert_eq!(next_week_start(MONDAY).as_deref(), Some("2026-11-02"));
        assert_eq!(next_week_start("2026-11-01").as_deref(), Some("2026-11-02"));
    }

    #[test]
    fn date_arithmetic_crosses_month_and_year() {
        assert_eq!(add_days(MONDAY, 7).as_deref(), Some("2026-11-02"));
        assert_eq!(add_days("2026-12-31", 1).as_deref(), Some("2027-01-01"));
        assert_eq!(add_days("2026-03-01", -1).as_deref(), Some("2026-02-28"));
        assert_eq!(add_days("2028-03-01", -1).as_deref(), Some("2028-02-29"));
        assert_eq!(days_between("2026-10-20", MONDAY), Some(6));
        assert_eq!(days_between("2026-10-26", "2026-10-20"), Some(-6));
        assert_eq!(add_days("nope", 1), None);
    }

    #[test]
    fn range_is_inclusive_and_never_reversed() {
        assert_eq!(
            date_range("2026-10-24", "2026-10-26"),
            Some(vec![
                "2026-10-24".to_string(),
                "2026-10-25".to_string(),
                MONDAY.to_string()
            ])
        );
        assert_eq!(date_range(MONDAY, MONDAY), Some(vec![MONDAY.to_string()]));
        assert_eq!(date_range("2026-10-26", "2026-10-25"), Some(Vec::new()));
        assert_eq!(date_range("2026-10-26", "bad"), None);
    }

    #[test]
    fn formats_back_to_the_same_string() {
        assert_eq!(format_date(20_752), MONDAY);
        assert_eq!(parse_date(&format_date(-1)), Some(-1));
    }
}
