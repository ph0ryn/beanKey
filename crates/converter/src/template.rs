use chrono::{DateTime as ChronoDateTime, Datelike, Duration, Local, Timelike};
use icu_calendar::Date;
use icu_calendar::cal::{Gregorian, Japanese};
use icu_datetime::input::{DateTime, Time};
use icu_datetime::pattern::{
    DateTimePattern, DayPeriodNameLength, FixedCalendarDateTimeNames, MonthNameLength,
    WeekdayNameLength, YearNameLength,
};
use icu_locale::Locale;
use rand::Rng;
use rand::prelude::IndexedRandom;
use writeable::TryWriteable;

pub fn expand_templates(text: &str) -> String {
    let mut rng = rand::rng();
    expand_templates_with(text, Local::now(), &mut rng)
}

fn expand_templates_with<R: Rng + ?Sized>(
    text: &str,
    now: ChronoDateTime<Local>,
    rng: &mut R,
) -> String {
    let mut output = replace_tags(text, "<date format=\"", |tag| {
        expand_date_template(tag, now)
    });
    output = replace_tags(&output, "<random type=\"", |tag| {
        expand_random_template(tag, rng)
    });
    output
}

fn replace_tags(
    text: &str,
    prefix: &str,
    mut replacement: impl FnMut(&str) -> Option<String>,
) -> String {
    let mut output = text.to_owned();
    let mut search_from = 0;
    while let Some(relative_start) = output[search_from..].find(prefix) {
        let start = search_from + relative_start;
        let Some(relative_end) = output[start..].find('>') else {
            break;
        };
        let end = start + relative_end + 1;
        if let Some(value) = replacement(&output[start..end]) {
            output.replace_range(start..end, &value);
            search_from = start + value.len();
        } else {
            search_from = end;
        }
    }
    output
}

fn expand_date_template(tag: &str, now: ChronoDateTime<Local>) -> Option<String> {
    let format = unescape(attribute(tag, "format")?);
    let calendar = attribute(tag, "type")?;
    let language = attribute(tag, "language")?;
    let delta = attribute(tag, "delta")?.parse::<i64>().unwrap_or(0);
    let delta_unit = attribute(tag, "deltaunit")?.parse::<i64>().unwrap_or(0);
    let duration = Duration::try_seconds(delta.checked_mul(delta_unit)?)?;
    let date = now.checked_add_signed(duration)?;
    format_date(&format, calendar, language, date)
}

macro_rules! format_with_calendar {
    ($calendar:ty, $format:expr, $locale:expr, $pattern:expr, $datetime:expr) => {{
        let mut names = FixedCalendarDateTimeNames::<$calendar>::try_new($locale.into()).ok()?;
        if let Some(length) = year_name_length($format) {
            names.include_year_names(length).ok()?;
        }
        if let Some(length) = month_name_length($format) {
            names.include_month_names(length).ok()?;
        }
        if let Some(length) = weekday_name_length($format) {
            names.include_weekday_names(length).ok()?;
        }
        if let Some(length) = day_period_name_length($format) {
            names.include_day_period_names(length).ok()?;
        }
        names
            .with_pattern_unchecked(&$pattern)
            .format($datetime)
            .try_write_to_string()
            .ok()
            .map(|value| value.into_owned())
    }};
}

fn format_date(
    format: &str,
    calendar: &str,
    language: &str,
    date: ChronoDateTime<Local>,
) -> Option<String> {
    let locale: Locale = language.replace('_', "-").parse().ok()?;
    let pattern: DateTimePattern = format.parse().ok()?;
    let iso_date = Date::try_new_iso(
        date.year(),
        u8::try_from(date.month()).ok()?,
        u8::try_from(date.day()).ok()?,
    )
    .ok()?;
    let time = Time::try_new(
        u8::try_from(date.hour()).ok()?,
        u8::try_from(date.minute()).ok()?,
        u8::try_from(date.second()).ok()?,
        date.nanosecond(),
    )
    .ok()?;

    match calendar {
        "western" => {
            let datetime = DateTime {
                date: iso_date.to_calendar(Gregorian),
                time,
            };
            format_with_calendar!(Gregorian, format, locale, pattern, &datetime)
        }
        "japanese" => {
            let datetime = DateTime {
                date: iso_date.to_calendar(Japanese::new()),
                time,
            };
            format_with_calendar!(Japanese, format, locale, pattern, &datetime)
        }
        _ => None,
    }
}

fn year_name_length(pattern: &str) -> Option<YearNameLength> {
    symbol_run(pattern, "G").and_then(|(_, width)| match width {
        1..=3 => Some(YearNameLength::Abbreviated),
        4 => Some(YearNameLength::Wide),
        5.. => Some(YearNameLength::Narrow),
        _ => None,
    })
}

fn month_name_length(pattern: &str) -> Option<MonthNameLength> {
    symbol_run(pattern, "ML").and_then(|(symbol, width)| match (symbol, width) {
        ('M', 3) => Some(MonthNameLength::Abbreviated),
        ('M', 4) => Some(MonthNameLength::Wide),
        ('M', 5..) => Some(MonthNameLength::Narrow),
        ('L', 3) => Some(MonthNameLength::StandaloneAbbreviated),
        ('L', 4) => Some(MonthNameLength::StandaloneWide),
        ('L', 5..) => Some(MonthNameLength::StandaloneNarrow),
        _ => None,
    })
}

fn weekday_name_length(pattern: &str) -> Option<WeekdayNameLength> {
    symbol_run(pattern, "Eec").and_then(|(symbol, width)| match (symbol, width) {
        ('E' | 'e', 1..=3) => Some(WeekdayNameLength::Abbreviated),
        ('E' | 'e', 4) => Some(WeekdayNameLength::Wide),
        ('E' | 'e', 5) => Some(WeekdayNameLength::Narrow),
        ('E' | 'e', 6..) => Some(WeekdayNameLength::Short),
        ('c', 3) => Some(WeekdayNameLength::StandaloneAbbreviated),
        ('c', 4) => Some(WeekdayNameLength::StandaloneWide),
        ('c', 5) => Some(WeekdayNameLength::StandaloneNarrow),
        ('c', 6..) => Some(WeekdayNameLength::StandaloneShort),
        _ => None,
    })
}

fn day_period_name_length(pattern: &str) -> Option<DayPeriodNameLength> {
    symbol_run(pattern, "abB").and_then(|(_, width)| match width {
        1..=3 => Some(DayPeriodNameLength::Abbreviated),
        4 => Some(DayPeriodNameLength::Wide),
        5.. => Some(DayPeriodNameLength::Narrow),
        _ => None,
    })
}

fn symbol_run(pattern: &str, symbols: &str) -> Option<(char, usize)> {
    let mut quoted = false;
    let mut characters = pattern.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\'' {
            if characters.peek() == Some(&'\'') {
                characters.next();
            } else {
                quoted = !quoted;
            }
            continue;
        }
        if !quoted && symbols.contains(character) {
            let mut width = 1;
            while characters.peek() == Some(&character) {
                characters.next();
                width += 1;
            }
            return Some((character, width));
        }
    }
    None
}

fn expand_random_template<R: Rng + ?Sized>(tag: &str, rng: &mut R) -> Option<String> {
    let value_type = attribute(tag, "type")?;
    let value = unescape(attribute(tag, "value")?);
    match value_type {
        "int" => {
            let (left, right) = value.split_once(',')?;
            let left = left.parse::<i64>().ok()?;
            let right = right.parse::<i64>().ok()?;
            (left <= right).then(|| rng.random_range(left..=right).to_string())
        }
        "double" => {
            let (left, right) = value.split_once(',')?;
            let left = left.parse::<f64>().ok()?;
            let right = right.parse::<f64>().ok()?;
            (left.is_finite() && right.is_finite() && left <= right)
                .then(|| rng.random_range(left..=right).to_string())
        }
        "string" => value
            .split(',')
            .collect::<Vec<_>>()
            .choose(rng)
            .map(|value| (*value).to_owned())
            .or_else(|| Some("データ無し".into())),
        _ => None,
    }
}

fn attribute<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=\"");
    let start = tag
        .split_ascii_whitespace()
        .find(|part| part.starts_with(&prefix))?;
    let value = start.strip_prefix(&prefix)?;
    value
        .strip_suffix("\">")
        .or_else(|| value.strip_suffix('"'))
}

fn unescape(value: &str) -> String {
    value
        .replace("\\d", "\"")
        .replace("\\s", " ")
        .replace("\\c", ",")
        .replace("\\t", "\t")
        .replace("\\n", "\n")
        .replace("\\0", "\0")
        .replace("\\b", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn fixed_now() -> ChronoDateTime<Local> {
        Local
            .with_ymd_and_hms(2025, 1, 15, 13, 4, 5)
            .single()
            .expect("fixed local time exists")
    }

    #[test]
    fn expands_western_and_japanese_date_templates() {
        let mut rng = StdRng::seed_from_u64(1);
        let western = "<date format=\"yyyy年MM月dd日(EEE)\\sa\\shh:mm:ss\" type=\"western\" language=\"ja_JP\" delta=\"0\" deltaunit=\"1\">";
        assert_eq!(
            expand_templates_with(western, fixed_now(), &mut rng),
            "2025年01月15日(水) 午後 01:04:05"
        );

        let japanese = "<date format=\"Gy年MM月dd日\" type=\"japanese\" language=\"ja_JP\" delta=\"1\" deltaunit=\"86400\">";
        assert_eq!(
            expand_templates_with(japanese, fixed_now(), &mut rng),
            "令和7年01月16日"
        );
    }

    #[test]
    fn leaves_out_of_range_date_templates_unchanged() {
        let mut rng = StdRng::seed_from_u64(1);
        let seconds_limit = i64::MAX / 1_000;
        for (delta, unit) in [
            (i64::MAX, 1),
            (i64::MIN, 1),
            (seconds_limit, 1),
            (seconds_limit + 1, 1),
            (-seconds_limit, 1),
            (-seconds_limit - 1, 1),
            (i64::MAX, 2),
            (i64::MIN, -1),
            (i64::MIN, 2),
        ] {
            let text = format!(
                "before/<date format=\"yyyy-MM-dd\" type=\"western\" language=\"ja_JP\" delta=\"{delta}\" deltaunit=\"{unit}\">/after"
            );
            assert_eq!(
                expand_templates_with(&text, fixed_now(), &mut rng),
                text,
                "delta={delta}, deltaunit={unit}"
            );
        }
    }

    #[test]
    fn expands_valid_dates_after_an_invalid_date_template() {
        let mut rng = StdRng::seed_from_u64(1);
        let invalid = "<date format=\"yyyy\" type=\"western\" language=\"ja_JP\" delta=\"9223372036854775807\" deltaunit=\"1\">";
        for (delta, unit, expected) in [
            (-1, 86_400, "2025-01-14"),
            (1, -86_400, "2025-01-14"),
            (i64::MAX, 0, "2025-01-15"),
            (i64::MIN, 0, "2025-01-15"),
        ] {
            let text = format!(
                "{invalid}/<date format=\"yyyy-MM-dd\" type=\"western\" language=\"ja_JP\" delta=\"{delta}\" deltaunit=\"{unit}\">"
            );
            assert_eq!(
                expand_templates_with(&text, fixed_now(), &mut rng),
                format!("{invalid}/{expected}")
            );
        }
    }

    #[test]
    fn expands_each_random_template_type_and_escaped_strings() {
        let mut rng = StdRng::seed_from_u64(7);
        let text = "<random type=\"int\" value=\"1,1\">/<random type=\"double\" value=\"0.5,0.5\">/<random type=\"string\" value=\"a\\cb\">";
        assert_eq!(
            expand_templates_with(text, fixed_now(), &mut rng),
            "1/0.5/a"
        );
    }

    #[test]
    fn leaves_invalid_templates_unchanged() {
        let mut rng = StdRng::seed_from_u64(1);
        let invalid = "<random type=\"int\" value=\"2,1\">";
        assert_eq!(
            expand_templates_with(invalid, fixed_now(), &mut rng),
            invalid
        );
    }
}
