use crate::kana::to_katakana;
use crate::{Candidate, ComposingCount, ComposingText, DictionaryEntry, DictionaryMetadata};

const GENERAL_NOUN_ID: u16 = 1285;
const PROPER_NOUN_ID: u16 = 1288;
const GENERAL_MEANING_ID: u16 = 501;
const YEAR_MEANING_ID: u16 = 237;

const EMAIL_DOMAINS: &[&str] = &[
    "@gmail.com",
    "@icloud.com",
    "@yahoo.co.jp",
    "@au.com",
    "@docomo.ne.jp",
    "@excite.co.jp",
    "@ezweb.ne.jp",
    "@googlemail.com",
    "@hotmail.co.jp",
    "@hotmail.com",
    "@i.softbank.jp",
    "@live.jp",
    "@me.com",
    "@mineo.jp",
    "@nifty.com",
    "@outlook.com",
    "@outlook.jp",
    "@softbank.ne.jp",
    "@yahoo.ne.jp",
    "@ybb.ne.jp",
    "@ymobile.ne.jp",
];

pub fn special_candidates(
    composing: &ComposingText,
    version_string: Option<&str>,
) -> Vec<Candidate> {
    let mut output = calendar_candidates(composing);
    output.extend(email_candidates(composing));
    output.extend(unicode_candidates(composing));
    output.extend(version_candidates(composing, version_string));
    output.extend(time_candidates(composing));
    output.extend(comma_separated_number_candidates(composing));
    output
}

pub fn typographical_candidates(composing: &ComposingText) -> Vec<Candidate> {
    let text = to_katakana(&composing.surface());
    if text.is_empty()
        || !text
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Vec::new();
    }
    let only_letters = text
        .chars()
        .all(|character| character.is_ascii_alphabetic());
    let mut values = vec![styled(&text, 119_743, 119_737, Some(120_734), &[])];
    if only_letters {
        values.push(styled(&text, 119_795, 119_789, None, &[('h', 'ℎ')]));
        values.push(styled(&text, 119_847, 119_841, None, &[]));
        values.push(styled(
            &text,
            119_899,
            119_893,
            None,
            &[
                ('B', 'ℬ'),
                ('E', 'ℰ'),
                ('F', 'ℱ'),
                ('H', 'ℋ'),
                ('I', 'ℐ'),
                ('L', 'ℒ'),
                ('M', 'ℳ'),
                ('R', 'ℛ'),
                ('e', 'ℯ'),
                ('g', 'ℊ'),
                ('o', 'ℴ'),
            ],
        ));
        values.push(styled(&text, 119_951, 119_945, None, &[]));
        values.push(styled(
            &text,
            120_003,
            119_997,
            None,
            &[('C', 'ℭ'), ('H', 'ℌ'), ('I', 'ℑ'), ('R', 'ℜ'), ('Z', 'ℨ')],
        ));
    }
    values.push(styled(
        &text,
        120_055,
        120_049,
        Some(120_744),
        &[
            ('C', 'ℂ'),
            ('H', 'ℍ'),
            ('N', 'ℕ'),
            ('P', 'ℙ'),
            ('Q', 'ℚ'),
            ('R', 'ℝ'),
            ('Z', 'ℤ'),
        ],
    ));
    if only_letters {
        values.push(styled(&text, 120_107, 120_101, None, &[]));
    }
    values.push(styled(&text, 120_159, 120_153, Some(120_754), &[]));
    values.push(styled(&text, 120_211, 120_205, Some(120_764), &[]));
    if only_letters {
        values.push(styled(&text, 120_263, 120_257, None, &[]));
        values.push(styled(&text, 120_315, 120_309, None, &[]));
    }
    values.push(styled(&text, 120_367, 120_361, Some(120_774), &[]));
    values
        .into_iter()
        .map(|value| {
            candidate(
                value,
                text.clone(),
                -15.0,
                ComposingCount::Input(composing.input().len()),
                PROPER_NOUN_ID,
                GENERAL_MEANING_ID,
                true,
            )
        })
        .collect()
}

fn styled(
    text: &str,
    uppercase_offset: u32,
    lowercase_offset: u32,
    number_offset: Option<u32>,
    exceptions: &[(char, char)],
) -> String {
    text.chars()
        .map(|character| {
            if let Some((_, replacement)) =
                exceptions.iter().find(|(source, _)| *source == character)
            {
                return *replacement;
            }
            let offset = if character.is_ascii_uppercase() {
                Some(uppercase_offset)
            } else if character.is_ascii_lowercase() {
                Some(lowercase_offset)
            } else if character.is_ascii_digit() {
                number_offset
            } else {
                None
            };
            offset
                .and_then(|offset| char::from_u32(u32::from(character) + offset))
                .unwrap_or(character)
        })
        .collect()
}

fn calendar_candidates(composing: &ComposingText) -> Vec<Candidate> {
    let ruby = to_katakana(&composing.surface());
    let input_count = ComposingCount::Input(composing.input().len());
    let mut output = Vec::new();
    if let Some(value) = japanese_era_to_western(&ruby) {
        output.push(candidate(
            value,
            ruby.clone(),
            -15.0,
            input_count.clone(),
            PROPER_NOUN_ID,
            GENERAL_MEANING_ID,
            true,
        ));
    }
    for (index, value) in western_to_japanese_era(&ruby).into_iter().enumerate() {
        output.push(candidate(
            value,
            ruby.clone(),
            -18.0 - index as f32,
            input_count.clone(),
            GENERAL_NOUN_ID,
            YEAR_MEANING_ID,
            true,
        ));
    }
    output
}

fn japanese_era_to_western(value: &str) -> Option<String> {
    let total_count = value.chars().count();
    match value {
        "メイジガンネン" => return Some("1868年".into()),
        "タイショウガンネン" => return Some("1912年".into()),
        "ショウワガンネン" => return Some("1926年".into()),
        "ヘイセイガンネン" => return Some("1989年".into()),
        "レイワガンネン" => return Some("2019年".into()),
        _ => {}
    }
    let value = value.strip_suffix("ネン")?;
    let (era, offset, allowed_counts): (&str, i32, &[usize]) = [
        ("ショウワ", 1925, &[7, 8][..]),
        ("ヘイセイ", 1988, &[7, 8][..]),
        ("レイワ", 2018, &[6, 7][..]),
        ("メイジ", 1867, &[6, 7][..]),
        ("タイショウ", 1911, &[8, 9][..]),
    ]
    .into_iter()
    .find(|(era, _, _)| value.starts_with(era))?;
    if !allowed_counts.contains(&total_count) {
        return None;
    }
    let year: i32 = value.strip_prefix(era)?.parse().ok()?;
    Some(format!("{}年", year + offset))
}

fn western_to_japanese_era(value: &str) -> Vec<String> {
    if !value.ends_with("ネン") {
        return Vec::new();
    }
    let Some(year) = value
        .chars()
        .take(4)
        .collect::<String>()
        .parse::<i32>()
        .ok()
    else {
        return Vec::new();
    };
    match year {
        1989 => vec!["平成元年".into(), "昭和64年".into()],
        2019 => vec!["令和元年".into(), "平成31年".into()],
        1926 => vec!["昭和元年".into(), "大正15年".into()],
        1912 => vec!["大正元年".into(), "明治45年".into()],
        1868 => vec!["明治元年".into(), "慶應4年".into()],
        1990..=2018 => vec![format!("平成{}年", year - 1988)],
        1927..=1988 => vec![format!("昭和{}年", year - 1925)],
        1869..=1911 => vec![format!("明治{}年", year - 1867)],
        1913..=1925 => vec![format!("大正{}年", year - 1911)],
        2020.. => vec![format!("令和{}年", year - 2018)],
        _ => Vec::new(),
    }
}

fn email_candidates(composing: &ComposingText) -> Vec<Candidate> {
    let surface = composing.surface();
    let Some((identifier, prefix)) = surface.rsplit_once('@') else {
        return Vec::new();
    };
    if !identifier.is_empty() && !is_english_sentence(identifier) {
        return Vec::new();
    }
    let ruby = to_katakana(&surface);
    let base = if identifier.is_empty() { -20.0 } else { -13.0 };
    EMAIL_DOMAINS
        .iter()
        .enumerate()
        .filter(|(_, domain)| domain.starts_with(&format!("@{prefix}")))
        .map(|(index, domain)| {
            candidate(
                format!("{identifier}{domain}"),
                ruby.clone(),
                base - index as f32,
                ComposingCount::Input(composing.input().len()),
                0,
                GENERAL_MEANING_ID,
                false,
            )
        })
        .collect()
}

fn unicode_candidates(composing: &ComposingText) -> Vec<Candidate> {
    let ruby = to_katakana(&composing.surface());
    for prefix in ["u", "U", "u+", "U+"] {
        if let Some(hex) = ruby.strip_prefix(prefix)
            && let Ok(number) = u32::from_str_radix(hex, 16)
            && let Some(character) = char::from_u32(number)
        {
            return vec![candidate(
                character.to_string(),
                ruby,
                -10.0,
                ComposingCount::Input(composing.input().len()),
                0,
                GENERAL_MEANING_ID,
                true,
            )];
        }
    }
    Vec::new()
}

fn version_candidates(composing: &ComposingText, version: Option<&str>) -> Vec<Candidate> {
    let ruby = to_katakana(&composing.surface());
    if ruby != "バージョン" {
        return Vec::new();
    }
    version
        .map(|version| {
            vec![candidate(
                version.into(),
                ruby,
                -30.0,
                ComposingCount::Input(composing.input().len()),
                PROPER_NOUN_ID,
                GENERAL_MEANING_ID,
                false,
            )]
        })
        .unwrap_or_default()
}

fn time_candidates(composing: &ComposingText) -> Vec<Candidate> {
    let value = composing.surface();
    if !value.chars().all(|character| character.is_ascii_digit()) {
        return Vec::new();
    }
    let time = match value.len() {
        3 => {
            let hour: u8 = value[..1].parse().expect("validated time hour");
            let minute: u8 = value[1..].parse().expect("validated time minute");
            (minute <= 59).then(|| format!("{hour}:{minute:02}"))
        }
        4 => {
            let hour: u8 = value[..2].parse().expect("validated time hour");
            let minute: u8 = value[2..].parse().expect("validated time minute");
            (hour <= 24 && minute <= 59).then(|| format!("{hour:02}:{minute:02}"))
        }
        _ => None,
    };
    time.map(|time| {
        vec![candidate(
            time,
            value,
            -10.0,
            ComposingCount::Surface(composing.surface_graphemes().len()),
            PROPER_NOUN_ID,
            GENERAL_MEANING_ID,
            true,
        )]
    })
    .unwrap_or_default()
}

fn comma_separated_number_candidates(composing: &ComposingText) -> Vec<Candidate> {
    let surface = composing.surface();
    let (negative, value) = surface
        .strip_prefix('-')
        .map_or((false, surface.as_str()), |value| (true, value));
    let parts: Vec<_> = value.split('.').collect();
    if parts.is_empty()
        || parts.len() > 2
        || parts.iter().any(|part| {
            part.is_empty() || !part.chars().all(|character| character.is_ascii_digit())
        })
        || parts[0].len() <= 3
    {
        return Vec::new();
    }
    let mut integer = String::new();
    for (index, character) in parts[0].chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            integer.push(',');
        }
        integer.push(character);
    }
    let mut result: String = integer.chars().rev().collect();
    if negative {
        result.insert(0, '-');
    }
    if parts.len() == 2 {
        result.push('.');
        result.push_str(parts[1]);
    }
    vec![candidate(
        result,
        to_katakana(&surface),
        -10.0,
        ComposingCount::Input(composing.input().len()),
        PROPER_NOUN_ID,
        GENERAL_MEANING_ID,
        true,
    )]
}

fn candidate(
    word: String,
    ruby: String,
    value: f32,
    composing_count: ComposingCount,
    class_id: u16,
    meaning_id: u16,
    learning_target: bool,
) -> Candidate {
    let entry = DictionaryEntry {
        word: word.clone(),
        ruby,
        left_id: class_id,
        right_id: class_id,
        meaning_id,
        base_value: value,
        adjustment: 0.0,
        metadata: DictionaryMetadata::default(),
    };
    Candidate::single(word, value, composing_count, meaning_id, vec![entry])
        .with_learning_target(learning_target)
}

fn is_english_sentence(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || "\n !'_<>[]{}*@`^|~=\"#$%&+(),-./:;?\\’".contains(character)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputStyle, InputTableRegistry};

    fn composing(value: &str) -> ComposingText {
        let tables = InputTableRegistry::new();
        let mut composing = ComposingText::new();
        composing.insert_str(value, InputStyle::Direct, &tables);
        composing
    }

    #[test]
    fn generates_each_default_special_candidate_family() {
        assert_eq!(
            special_candidates(&composing("2019ねん"), None)[0].text,
            "令和元年"
        );
        assert_eq!(
            special_candidates(&composing("me@out"), None)[0].text,
            "me@outlook.com"
        );
        assert_eq!(
            special_candidates(&composing("U+1F600"), None)[0].text,
            "😀"
        );
        assert_eq!(
            special_candidates(&composing("1234"), None)[0].text,
            "12:34"
        );
        assert!(
            special_candidates(&composing("1234"), None)
                .iter()
                .any(|candidate| candidate.text == "1,234")
        );
        assert_eq!(
            special_candidates(&composing("ばーじょん"), Some("beanKey 0.1"))[0].text,
            "beanKey 0.1"
        );
    }

    #[test]
    fn generates_the_optional_fixed_typography_styles() {
        let letters = typographical_candidates(&composing("Beh"));
        assert_eq!(letters.len(), 13);
        assert_eq!(letters[0].text, "𝐁𝐞𝐡");
        assert_eq!(letters[1].text, "𝐵𝑒ℎ");
        assert_eq!(letters[3].text, "ℬℯ𝒽");

        let alphanumeric = typographical_candidates(&composing("Az0"));
        assert_eq!(alphanumeric.len(), 5);
        assert_eq!(
            alphanumeric
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["𝐀𝐳𝟎", "𝔸𝕫𝟘", "𝖠𝗓𝟢", "𝗔𝘇𝟬", "𝙰𝚣𝟶"]
        );
        assert!(typographical_candidates(&composing("日本語")).is_empty());
    }
}
