//! 「此刻」从哪来。perch-core 的日界要的是 UTC 毫秒 + UTC 偏移秒，这两个值只能问 Windows：
//! 核心里不读时钟、不引时区库（DESIGN §5.4），手机端将来传自己的值进来就是同一套口径。

use std::time::{SystemTime, UNIX_EPOCH};

use perch_core::Now;
use windows_sys::Win32::System::Time::{
    GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION,
};

/// `GetDynamicTimeZoneInformation` 的返回值里这两个态要区分，windows-sys 0.59 没导出常量，
/// 按 Windows 头里的值写在这里（返回类型是 `TIME_ZONE_INFORMATION_FLAGS`，u32）。
const TIME_ZONE_ID_STANDARD: u32 = 1;
const TIME_ZONE_ID_DAYLIGHT: u32 = 2;

/// 现在。每次写入都重新取一次，不用启动时缓存的那个值——卡片可以开好几个星期。
pub fn now() -> Now {
    Now {
        instant_ms: now_ms(),
        offset_secs: local_offset_secs(),
    }
}

fn now_ms() -> i64 {
    // 系统时钟被调到 1970 年之前是可能的（ BIOS 抽风 ），那种情况下 0 比 panic 好。
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// 本地相对 UTC 的偏移秒数。
///
/// Windows 的 `Bias` 是「本地 = UTC 减 Bias 分钟」，符号和我们要的相反，所以取负。
/// DST 生效时还要再加 `DaylightBias`，否则夏天会整整差一小时——04:00 日界跟着错一天。
/// 读不到（`TIME_ZONE_ID_UNKNOWN` / `FAILED`）就只用 `Bias`，再退一步按 UTC：
/// 日期可能差几个小时，但不该让卡片打不开。
fn local_offset_secs() -> i32 {
    // 全整数字段，清零是合法值；bindings 没给 Default，就不假装它有。
    let mut info: DYNAMIC_TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let kind = unsafe { GetDynamicTimeZoneInformation(&mut info) };
    let extra = if kind == TIME_ZONE_ID_DAYLIGHT {
        info.DaylightBias
    } else if kind == TIME_ZONE_ID_STANDARD {
        info.StandardBias
    } else {
        0
    };
    -(info.Bias + extra) * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只验「问到了合理范围内的值」，不验具体偏移——那是本机状态，口径由 perch-core 的单测负责。
    #[test]
    fn clock_answers_within_human_ranges() {
        let instant = now();
        assert!(instant.instant_ms > 1_700_000_000_000);
        assert!(instant.offset_secs.abs() <= 14 * 3600);
        // 日界函数必须能算出一个日期，传错类型这里就编译不过，传错值这里会露出来。
        assert_eq!(instant.date().len(), 10);
        assert_eq!(instant.stamp().len(), 29);
    }
}
