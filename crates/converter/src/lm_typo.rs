use std::collections::HashMap;

use unicode_segmentation::UnicodeSegmentation;

use crate::kana::to_katakana;
use crate::zenz::tokenize_for_model;
use crate::{
    ComposingText, InputPiece, InputStyle, InputTable, InputTableRegistry, KeyElement,
    ValueElement, ZenzInferenceError, ZenzLanguageModel, ZenzPromptBuilder,
};

#[derive(Clone, Debug, PartialEq)]
pub struct LmTypoConfig {
    pub beam_size: usize,
    pub top_k: usize,
    pub n_best: usize,
    pub max_steps: Option<usize>,
    pub substitution_cost: f32,
    pub deletion_cost: f32,
    pub transposition_cost: f32,
}

impl Default for LmTypoConfig {
    fn default() -> Self {
        Self {
            beam_size: 32,
            top_k: 64,
            n_best: 5,
            max_steps: None,
            substitution_cost: 2.0,
            deletion_cost: 3.0,
            transposition_cost: 2.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LmTypoCandidate {
    pub corrected_input: String,
    pub converted_text: String,
    pub score: f32,
    pub lm_score: f32,
    pub channel_cost: f32,
    pub prominence: f32,
}

#[derive(Clone)]
struct Hypothesis {
    corrected: Vec<String>,
    emitted_text: String,
    emitted_tokens: Vec<i32>,
    observed_position: usize,
    previous_input: Option<String>,
    pending: String,
    proxy_log_probability: f32,
    channel_cost: f32,
    lm_score: f32,
    score: f32,
}

#[derive(Clone, Copy)]
enum Topology {
    MacQwerty,
    IosQwerty,
    IosFlickTenkey,
}

struct GenerationMode<'a> {
    table: Option<&'a InputTable>,
    topology: Topology,
    use_surface: bool,
    filter_with_lm: bool,
}

struct LmScorer<'a> {
    model: &'a mut dyn ZenzLanguageModel,
    prompt_tokens: Vec<i32>,
    token_cache: HashMap<String, Vec<i32>>,
    distribution_cache: HashMap<Vec<i32>, Vec<f32>>,
    pending_token_cache: HashMap<String, Vec<i32>>,
    pending_score_cache: HashMap<(Vec<i32>, String), f32>,
}

impl<'a> LmScorer<'a> {
    fn new(model: &'a mut dyn ZenzLanguageModel, prompt: &str) -> Result<Self, ZenzInferenceError> {
        let prompt_tokens = tokenize_for_model(model, prompt, false)?;
        Ok(Self {
            model,
            prompt_tokens,
            token_cache: HashMap::new(),
            distribution_cache: HashMap::new(),
            pending_token_cache: HashMap::new(),
            pending_score_cache: HashMap::new(),
        })
    }

    fn tokenize(&mut self, text: &str) -> Result<Vec<i32>, ZenzInferenceError> {
        if let Some(tokens) = self.token_cache.get(text) {
            return Ok(tokens.clone());
        }
        let tokens = tokenize_for_model(self.model, text, false)?;
        self.token_cache.insert(text.to_owned(), tokens.clone());
        Ok(tokens)
    }

    fn next_log_probabilities(
        &mut self,
        emitted_tokens: &[i32],
    ) -> Result<Vec<f32>, ZenzInferenceError> {
        if let Some(probabilities) = self.distribution_cache.get(emitted_tokens) {
            return Ok(probabilities.clone());
        }
        let mut tokens = self.prompt_tokens.clone();
        tokens.extend_from_slice(emitted_tokens);
        let logits = self.model.next_logits(&tokens)?;
        if logits.len() != self.model.vocabulary_size() || logits.is_empty() {
            return Err(ZenzInferenceError::Message(format!(
                "model returned {} logits for a vocabulary of {}",
                logits.len(),
                self.model.vocabulary_size()
            )));
        }
        let maximum = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let sum = logits
            .iter()
            .map(|value| (*value - maximum).exp())
            .sum::<f32>();
        let normalization = maximum + sum.ln();
        let probabilities: Vec<_> = logits
            .into_iter()
            .map(|value| value - normalization)
            .collect();
        self.distribution_cache
            .insert(emitted_tokens.to_vec(), probabilities.clone());
        Ok(probabilities)
    }

    fn append_and_score(
        &mut self,
        emitted_tokens: &[i32],
        append_text: &str,
    ) -> Result<Option<(Vec<i32>, f32)>, ZenzInferenceError> {
        let append_tokens = self.tokenize(append_text)?;
        let mut tokens = emitted_tokens.to_vec();
        let mut score = 0.0;
        for token in append_tokens {
            let probabilities = self.next_log_probabilities(&tokens)?;
            let Some(index) = usize::try_from(token)
                .ok()
                .filter(|index| *index < probabilities.len())
            else {
                return Ok(None);
            };
            score += probabilities[index];
            tokens.push(token);
        }
        Ok(Some((tokens, score)))
    }

    fn top_characters(
        &mut self,
        emitted_tokens: &[i32],
        count: usize,
    ) -> Result<Vec<String>, ZenzInferenceError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let probabilities = self.next_log_probabilities(emitted_tokens)?;
        let mut token_ids: Vec<_> = (0..probabilities.len()).collect();
        token_ids.sort_by(|left, right| probabilities[*right].total_cmp(&probabilities[*left]));
        let mut result = Vec::new();
        for token_id in token_ids.into_iter().take(count.saturating_mul(4)) {
            let Ok(token_id) = i32::try_from(token_id) else {
                continue;
            };
            let Ok(piece) = self.model.token_to_piece(token_id) else {
                continue;
            };
            let Ok(piece) = String::from_utf8(piece) else {
                continue;
            };
            let graphemes: Vec<_> = piece.graphemes(true).collect();
            if graphemes.len() == 1 && !result.iter().any(|value| value == graphemes[0]) {
                result.push(graphemes[0].to_owned());
                if result.len() == count {
                    break;
                }
            }
        }
        Ok(result)
    }

    fn pending_proxy_log_probability(
        &mut self,
        pending: &str,
        emitted_tokens: &[i32],
        table: Option<&InputTable>,
    ) -> Result<Option<f32>, ZenzInferenceError> {
        if pending.is_empty() {
            return Ok(Some(0.0));
        }
        let Some(table) = table else {
            return Ok(None);
        };
        let cache_key = (emitted_tokens.to_vec(), pending.to_owned());
        if let Some(score) = self.pending_score_cache.get(&cache_key) {
            return Ok(Some(*score));
        }
        let first_tokens = if let Some(tokens) = self.pending_token_cache.get(pending) {
            tokens.clone()
        } else {
            let mut tokens = Vec::new();
            for display in possible_next_displays(pending, table) {
                let Some(first) = display.graphemes(true).next() else {
                    continue;
                };
                let encoded = self.tokenize(first)?;
                if encoded.len() == 1 && !tokens.contains(&encoded[0]) {
                    tokens.push(encoded[0]);
                }
            }
            tokens.sort_unstable();
            self.pending_token_cache
                .insert(pending.to_owned(), tokens.clone());
            tokens
        };
        if first_tokens.is_empty() {
            return Ok(None);
        }
        let probabilities = self.next_log_probabilities(emitted_tokens)?;
        let values: Vec<_> = first_tokens
            .into_iter()
            .filter_map(|token| usize::try_from(token).ok())
            .filter_map(|index| probabilities.get(index).copied())
            .collect();
        let maximum = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if !maximum.is_finite() {
            return Ok(None);
        }
        let sum = values
            .into_iter()
            .map(|value| (value - maximum).exp())
            .sum::<f32>();
        if sum <= 0.0 {
            return Ok(None);
        }
        let score = maximum + sum.ln();
        self.pending_score_cache.insert(cache_key, score);
        Ok(Some(score))
    }
}

pub fn experimental_typo_correction(
    model: &mut dyn ZenzLanguageModel,
    left_context: &str,
    composing: &ComposingText,
    input_style: &InputStyle,
    tables: &InputTableRegistry,
    config: &LmTypoConfig,
) -> Result<Vec<LmTypoCandidate>, ZenzInferenceError> {
    let config = LmTypoConfig {
        beam_size: config.beam_size.max(1),
        top_k: config.top_k.max(1),
        n_best: config.n_best.max(1),
        ..config.clone()
    };
    let mode = generation_mode(input_style, tables);
    let observed = observed_input(composing, mode.use_surface);
    if observed.is_empty() {
        return Ok(Vec::new());
    }

    let prompt = ZenzPromptBuilder::typo_correction_prefix(left_context);
    let mut scorer = LmScorer::new(model, &prompt)?;
    let initial = Hypothesis {
        corrected: Vec::new(),
        emitted_text: String::new(),
        emitted_tokens: Vec::new(),
        observed_position: 0,
        previous_input: None,
        pending: String::new(),
        proxy_log_probability: 0.0,
        channel_cost: 0.0,
        lm_score: 0.0,
        score: 0.0,
    };
    let mut beam = vec![initial.clone()];
    let max_steps = config.max_steps.unwrap_or(observed.len() * 2 + 8);

    for _ in 0..max_steps {
        if beam
            .iter()
            .all(|hypothesis| hypothesis.observed_position == observed.len())
        {
            break;
        }
        let mut expanded = Vec::new();
        for hypothesis in &beam {
            if hypothesis.observed_position == observed.len() {
                expanded.push(hypothesis.clone());
                continue;
            }
            expand_hypothesis(
                &mut scorer,
                &observed,
                hypothesis,
                &mode,
                &config,
                &mut expanded,
            )?;
        }
        if expanded.is_empty() {
            break;
        }
        expanded.sort_by(compare_hypotheses);
        expanded.truncate(config.beam_size);
        beam = expanded;
    }

    let mut completed = Vec::new();
    for hypothesis in beam {
        if let Some(hypothesis) =
            complete_hypothesis(&mut scorer, hypothesis, &observed, mode.table)?
        {
            completed.push(hypothesis);
        }
    }
    beam = completed;
    if let Some(original) = complete_hypothesis(&mut scorer, initial, &observed, mode.table)? {
        beam.push(original);
    }

    let mut unique = HashMap::<String, LmTypoCandidate>::new();
    for hypothesis in beam {
        let corrected_input = hypothesis.corrected.concat();
        let converted_text = format!("{}{}", hypothesis.emitted_text, hypothesis.pending);
        let candidate = LmTypoCandidate {
            corrected_input: corrected_input.clone(),
            converted_text,
            score: hypothesis.score,
            lm_score: hypothesis.lm_score,
            channel_cost: hypothesis.channel_cost,
            prominence: 0.0,
        };
        if unique
            .get(&corrected_input)
            .is_none_or(|existing| existing.score < candidate.score)
        {
            unique.insert(corrected_input, candidate);
        }
    }
    let mut candidates: Vec<_> = unique.into_values().collect();
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.corrected_input.cmp(&right.corrected_input))
    });
    candidates.truncate(config.n_best);
    if let Some(best) = candidates.first().map(|candidate| candidate.score) {
        for candidate in &mut candidates {
            candidate.prominence = (candidate.score - best).exp();
        }
    }
    Ok(candidates)
}

#[allow(clippy::too_many_arguments)]
fn expand_hypothesis(
    scorer: &mut LmScorer<'_>,
    observed: &[String],
    hypothesis: &Hypothesis,
    mode: &GenerationMode<'_>,
    config: &LmTypoConfig,
    expanded: &mut Vec<Hypothesis>,
) -> Result<(), ZenzInferenceError> {
    let position = hypothesis.observed_position;
    let current = &observed[position];
    let mut targets = vec![(current.clone(), 0.0)];
    let mut substitutions = neighbors(current, mode.topology);
    if mode.filter_with_lm {
        let likely = scorer.top_characters(&hypothesis.emitted_tokens, config.top_k)?;
        substitutions.retain(|(candidate, _)| likely.contains(candidate));
    }
    targets.extend(
        substitutions
            .into_iter()
            .map(|(target, distance)| (target, config.substitution_cost * distance)),
    );

    for (target, channel_addition) in targets {
        add_advance(
            scorer,
            hypothesis,
            observed,
            mode.table,
            vec![target],
            1,
            channel_addition,
            expanded,
        )?;
    }

    if position + 1 < observed.len()
        && hypothesis.previous_input.as_ref().is_some_and(|previous| {
            neighbors(previous, mode.topology)
                .iter()
                .any(|(neighbor, _)| neighbor == current)
        })
    {
        let distance = neighbors(
            hypothesis.previous_input.as_deref().unwrap_or_default(),
            mode.topology,
        )
        .into_iter()
        .find_map(|(neighbor, distance)| (neighbor == *current).then_some(distance))
        .unwrap_or(1.0);
        let mut next = hypothesis.clone();
        next.observed_position += 1;
        next.channel_cost += config.deletion_cost * distance;
        next.lm_score = hypothesis.lm_score - hypothesis.proxy_log_probability;
        if let Some(proxy) =
            scorer.pending_proxy_log_probability(&next.pending, &next.emitted_tokens, mode.table)?
        {
            next.proxy_log_probability = proxy;
            next.lm_score += proxy;
            next.score = next.lm_score - next.channel_cost;
            expanded.push(next);
        }
    }

    if position + 1 < observed.len() && observed[position] != observed[position + 1] {
        add_advance(
            scorer,
            hypothesis,
            observed,
            mode.table,
            vec![observed[position + 1].clone(), observed[position].clone()],
            2,
            config.transposition_cost,
            expanded,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn add_advance(
    scorer: &mut LmScorer<'_>,
    parent: &Hypothesis,
    observed: &[String],
    table: Option<&InputTable>,
    true_sequence: Vec<String>,
    observed_count: usize,
    channel_addition: f32,
    expanded: &mut Vec<Hypothesis>,
) -> Result<(), ZenzInferenceError> {
    let Some(last) = true_sequence.last() else {
        return Ok(());
    };
    let mut pending = parent.pending.clone();
    let mut emitted = String::new();
    for character in &true_sequence {
        let consumed = consume_with_emission(&pending, character, table);
        emitted.push_str(&consumed.0);
        pending = consumed.1;
    }
    let final_observed_index = parent.observed_position + observed_count - 1;
    if final_observed_index == observed.len() - 1
        && !pending.is_empty()
        && (last != &observed[final_observed_index] || channel_addition > 0.0)
    {
        return Ok(());
    }
    if let Some(next) = evaluate_advance(
        scorer,
        parent,
        true_sequence,
        observed_count,
        channel_addition,
        emitted,
        pending,
        table,
    )? {
        expanded.push(next);
    }
    Ok(())
}

fn compare_hypotheses(left: &Hypothesis, right: &Hypothesis) -> std::cmp::Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then_with(|| left.corrected.cmp(&right.corrected))
}

#[allow(clippy::too_many_arguments)]
fn evaluate_advance(
    scorer: &mut LmScorer<'_>,
    parent: &Hypothesis,
    corrected_append: Vec<String>,
    observed_count: usize,
    channel_addition: f32,
    emitted: String,
    pending: String,
    table: Option<&InputTable>,
) -> Result<Option<Hypothesis>, ZenzInferenceError> {
    if !parent.proxy_log_probability.is_finite() {
        return Ok(None);
    }
    let base_lm_score = parent.lm_score - parent.proxy_log_probability;
    let Some((emitted_tokens, emitted_score)) =
        scorer.append_and_score(&parent.emitted_tokens, &emitted)?
    else {
        return Ok(None);
    };
    let Some(proxy) = scorer.pending_proxy_log_probability(&pending, &emitted_tokens, table)?
    else {
        if pending.is_empty() {
            unreachable!("empty pending input always has a zero proxy score");
        }
        return Ok(None);
    };
    let mut next = parent.clone();
    next.corrected.extend(corrected_append.iter().cloned());
    next.emitted_text.push_str(&emitted);
    next.emitted_tokens = emitted_tokens;
    next.observed_position += observed_count;
    next.previous_input = corrected_append.last().cloned();
    next.pending = pending;
    next.proxy_log_probability = proxy;
    next.channel_cost += channel_addition;
    next.lm_score = base_lm_score + emitted_score + proxy;
    next.score = next.lm_score - next.channel_cost;
    Ok(Some(next))
}

fn complete_hypothesis(
    scorer: &mut LmScorer<'_>,
    mut hypothesis: Hypothesis,
    observed: &[String],
    table: Option<&InputTable>,
) -> Result<Option<Hypothesis>, ZenzInferenceError> {
    if hypothesis.observed_position >= observed.len() {
        return Ok(Some(hypothesis));
    }
    if !hypothesis.proxy_log_probability.is_finite() {
        return Ok(None);
    }
    let base_lm_score = hypothesis.lm_score - hypothesis.proxy_log_probability;
    let mut newly_emitted = String::new();
    while hypothesis.observed_position < observed.len() {
        let current = &observed[hypothesis.observed_position];
        let consumed = consume_with_emission(&hypothesis.pending, current, table);
        newly_emitted.push_str(&consumed.0);
        hypothesis.pending = consumed.1;
        hypothesis.corrected.push(current.clone());
        hypothesis.previous_input = Some(current.clone());
        hypothesis.observed_position += 1;
    }
    let Some((emitted_tokens, emitted_score)) =
        scorer.append_and_score(&hypothesis.emitted_tokens, &newly_emitted)?
    else {
        return Ok(None);
    };
    let Some(proxy) =
        scorer.pending_proxy_log_probability(&hypothesis.pending, &emitted_tokens, table)?
    else {
        return Ok(None);
    };
    hypothesis.emitted_text.push_str(&newly_emitted);
    hypothesis.emitted_tokens = emitted_tokens;
    hypothesis.proxy_log_probability = proxy;
    hypothesis.lm_score = base_lm_score + emitted_score + proxy;
    hypothesis.score = hypothesis.lm_score - hypothesis.channel_cost;
    Ok(Some(hypothesis))
}

fn generation_mode<'a>(
    input_style: &InputStyle,
    tables: &'a InputTableRegistry,
) -> GenerationMode<'a> {
    match input_style {
        InputStyle::RomanToKana => GenerationMode {
            table: tables.resolve(input_style),
            topology: Topology::MacQwerty,
            use_surface: false,
            filter_with_lm: false,
        },
        InputStyle::Mapped(_) => {
            let table = tables.resolve(input_style);
            let qwerty = table.is_some_and(|table| !table.possible_nexts("").is_empty());
            GenerationMode {
                table,
                topology: if qwerty {
                    Topology::IosQwerty
                } else {
                    Topology::IosFlickTenkey
                },
                use_surface: !qwerty,
                filter_with_lm: !qwerty,
            }
        }
        InputStyle::Direct => GenerationMode {
            table: None,
            topology: Topology::IosFlickTenkey,
            use_surface: true,
            filter_with_lm: true,
        },
    }
}

fn observed_input(composing: &ComposingText, use_surface: bool) -> Vec<String> {
    if use_surface {
        return UnicodeSegmentation::graphemes(to_katakana(&composing.surface()).as_str(), true)
            .map(str::to_owned)
            .collect();
    }
    composing
        .input()
        .iter()
        .filter_map(|element| match &element.piece {
            InputPiece::Character(character) => {
                character.graphemes(true).next().map(str::to_lowercase)
            }
            InputPiece::Key {
                intention, input, ..
            } => intention
                .as_deref()
                .unwrap_or(input)
                .graphemes(true)
                .next()
                .map(str::to_lowercase),
            InputPiece::CompositionSeparator => None,
        })
        .collect()
}

fn apply_input_table(input: &str, table: &InputTable) -> String {
    to_katakana(
        &input.graphemes(true).fold(String::new(), |current, piece| {
            table.applied(&current, InputPiece::character(piece))
        }),
    )
}

fn consume_with_emission(
    pending: &str,
    new_character: &str,
    table: Option<&InputTable>,
) -> (String, String) {
    let Some(table) = table else {
        return (new_character.to_owned(), String::new());
    };
    let raw = format!("{pending}{new_character}");
    let converted = apply_input_table(&raw, table);
    let next_pending = pending_suffix(&raw, &converted, table);
    if next_pending.is_empty() {
        return (converted, String::new());
    }
    let pending_count = next_pending.graphemes(true).count();
    let converted_count = converted.graphemes(true).count();
    if converted_count < pending_count {
        return (String::new(), next_pending);
    }
    let emitted = converted
        .graphemes(true)
        .take(converted_count - pending_count)
        .collect();
    (emitted, next_pending)
}

fn pending_suffix(raw: &str, converted: &str, table: &InputTable) -> String {
    let graphemes: Vec<_> = raw.graphemes(true).collect();
    for length in (1..=graphemes.len()).rev() {
        let suffix = graphemes[graphemes.len() - length..].concat();
        if !has_continuation(&suffix, table) {
            continue;
        }
        let display = apply_input_table(&suffix, table);
        if display == suffix && converted.ends_with(&display) {
            return suffix;
        }
    }
    String::new()
}

fn has_continuation(pending: &str, table: &InputTable) -> bool {
    if !table.possible_nexts(pending).is_empty() {
        return true;
    }
    let pending: Vec<_> = pending.graphemes(true).collect();
    table.entries().any(|(key, _)| {
        key.len() > pending.len()
            && key.iter().zip(&pending).all(|(element, character)| {
                matches!(element, KeyElement::Piece(InputPiece::Character(value)) if value == character)
            })
    })
}

fn possible_next_displays(pending: &str, table: &InputTable) -> Vec<String> {
    let mut displays = table.possible_nexts(pending);
    let pending: Vec<_> = pending.graphemes(true).collect();
    for (key, value) in table.entries() {
        let Some(any_index) = key
            .iter()
            .position(|element| matches!(element, KeyElement::Any))
        else {
            continue;
        };
        if any_index != key.len() - 1 || any_index != pending.len() {
            continue;
        }
        let prefix_matches = key[..any_index]
            .iter()
            .zip(&pending)
            .all(|(element, character)| {
                matches!(element, KeyElement::Piece(InputPiece::Character(value)) if value == character)
            });
        if !prefix_matches {
            continue;
        }
        let output: String = value
            .iter()
            .take_while(|element| !matches!(element, ValueElement::Any))
            .filter_map(|element| match element {
                ValueElement::Character(character) => Some(character.as_str()),
                ValueElement::Any => None,
            })
            .collect();
        if !output.is_empty() {
            displays.push(to_katakana(&output));
        }
    }
    displays.sort();
    displays.dedup();
    displays
}

fn neighbors(character: &str, topology: Topology) -> Vec<(String, f32)> {
    match topology {
        Topology::MacQwerty => coordinate_neighbors(character, MAC_QWERTY),
        Topology::IosQwerty => coordinate_neighbors(character, IOS_QWERTY),
        Topology::IosFlickTenkey => tenkey_neighbors(character),
    }
}

fn coordinate_neighbors(character: &str, coordinates: &[(&str, f32, f32)]) -> Vec<(String, f32)> {
    let Some((_, x, y)) = coordinates.iter().find(|(key, _, _)| *key == character) else {
        return Vec::new();
    };
    let mut result: Vec<_> = coordinates
        .iter()
        .filter_map(|(key, target_x, target_y)| {
            if *key == character {
                return None;
            }
            let distance = ((*x - *target_x).powi(2) + (*y - *target_y).powi(2)).sqrt();
            (distance <= 1.65).then(|| ((*key).to_owned(), distance))
        })
        .collect();
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

fn tenkey_neighbors(character: &str) -> Vec<(String, f32)> {
    const GROUPS: [&str; 16] = [
        "アイウエオ",
        "カキクケコ",
        "ガギグゲゴ",
        "サシスセソ",
        "ザジズゼゾ",
        "タチツテト",
        "ダヂヅデド",
        "ナニヌネノ",
        "ハヒフヘホ",
        "バビブベボ",
        "パピプペポ",
        "マミムメモ",
        "ヤユヨ",
        "ャュョ",
        "ラリルレロ",
        "ワヲンー",
    ];
    GROUPS
        .iter()
        .find(|group| group.contains(character))
        .map(|group| {
            UnicodeSegmentation::graphemes(*group, true)
                .filter(|neighbor| *neighbor != character)
                .map(|neighbor| (neighbor.to_owned(), 1.0))
                .collect()
        })
        .unwrap_or_default()
}

const MAC_QWERTY: &[(&str, f32, f32)] = &[
    ("1", -1.0, 0.0),
    ("2", 0.25, 0.0),
    ("3", 1.25, 0.0),
    ("4", 2.25, 0.0),
    ("5", 3.25, 0.0),
    ("6", 4.25, 0.0),
    ("7", 5.25, 0.0),
    ("8", 6.25, 0.0),
    ("9", 7.25, 0.0),
    ("0", 8.25, 0.0),
    ("-", 9.25, 0.0),
    ("^", 10.25, 0.0),
    ("q", 0.0, 1.0),
    ("w", 1.0, 1.0),
    ("e", 2.0, 1.0),
    ("r", 3.0, 1.0),
    ("t", 4.0, 1.0),
    ("y", 5.0, 1.0),
    ("u", 6.0, 1.0),
    ("i", 7.0, 1.0),
    ("o", 8.0, 1.0),
    ("p", 9.0, 1.0),
    ("@", 10.0, 1.0),
    ("[", 11.0, 1.0),
    ("a", 0.25, 2.0),
    ("s", 1.25, 2.0),
    ("d", 2.25, 2.0),
    ("f", 3.25, 2.0),
    ("g", 4.25, 2.0),
    ("h", 5.25, 2.0),
    ("j", 6.25, 2.0),
    ("k", 7.25, 2.0),
    ("l", 8.25, 2.0),
    (";", 9.25, 2.0),
    ("]", 10.25, 2.0),
    ("z", 0.8, 3.0),
    ("x", 1.8, 3.0),
    ("c", 2.8, 3.0),
    ("v", 3.8, 3.0),
    ("b", 4.8, 3.0),
    ("n", 5.8, 3.0),
    ("m", 6.8, 3.0),
    (",", 7.8, 3.0),
    (".", 8.8, 3.0),
    ("/", 9.8, 3.0),
    ("_", 10.8, 3.0),
];

const IOS_QWERTY: &[(&str, f32, f32)] = &[
    ("q", 0.0, 1.0),
    ("w", 1.0, 1.0),
    ("e", 2.0, 1.0),
    ("r", 3.0, 1.0),
    ("t", 4.0, 1.0),
    ("y", 5.0, 1.0),
    ("u", 6.0, 1.0),
    ("i", 7.0, 1.0),
    ("o", 8.0, 1.0),
    ("p", 9.0, 1.0),
    ("a", 0.5, 2.5),
    ("s", 1.5, 2.5),
    ("d", 2.5, 2.5),
    ("f", 3.5, 2.5),
    ("g", 4.5, 2.5),
    ("h", 5.5, 2.5),
    ("j", 6.5, 2.5),
    ("k", 7.5, 2.5),
    ("l", 8.5, 2.5),
    ("z", 1.5, 4.0),
    ("x", 2.5, 4.0),
    ("c", 3.5, 4.0),
    ("v", 4.5, 4.0),
    ("b", 5.5, 4.0),
    ("n", 6.5, 4.0),
    ("m", 7.5, 4.0),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct TestModel {
        pieces: Vec<Vec<u8>>,
        token_ids: HashMap<String, i32>,
        preferred: i32,
    }

    impl TestModel {
        fn new(values: &[&str], preferred: &str) -> Self {
            let pieces: Vec<_> = values
                .iter()
                .map(|value| value.as_bytes().to_vec())
                .collect();
            let token_ids: HashMap<String, i32> = values
                .iter()
                .enumerate()
                .map(|(index, value)| (value.to_string(), index as i32))
                .collect();
            let preferred = token_ids[preferred];
            Self {
                pieces,
                token_ids,
                preferred,
            }
        }
    }

    impl ZenzLanguageModel for TestModel {
        fn vocabulary_size(&self) -> usize {
            self.pieces.len()
        }
        fn eos_token(&self) -> i32 {
            0
        }
        fn tokenize(
            &mut self,
            text: &str,
            _add_special: bool,
        ) -> Result<Vec<i32>, ZenzInferenceError> {
            Ok(text
                .graphemes(true)
                .filter_map(|piece| self.token_ids.get(piece).copied())
                .collect())
        }
        fn token_to_piece(&mut self, token: i32) -> Result<Vec<u8>, ZenzInferenceError> {
            Ok(self.pieces[token as usize].clone())
        }
        fn next_logits(&mut self, _tokens: &[i32]) -> Result<Vec<f32>, ZenzInferenceError> {
            let mut logits = vec![-8.0; self.pieces.len()];
            logits[self.preferred as usize] = 8.0;
            Ok(logits)
        }
    }

    fn composing(value: &str, style: InputStyle) -> (ComposingText, InputTableRegistry) {
        let tables = InputTableRegistry::new();
        let mut composing = ComposingText::new();
        composing.insert_str(value, style, &tables);
        (composing, tables)
    }

    #[test]
    fn direct_input_uses_zenz_to_rank_tenkey_substitutions() {
        let (composing, tables) = composing("か", InputStyle::Direct);
        let mut model = TestModel::new(&["\u{ee00}", "カ", "キ", "ク", "ケ", "コ"], "キ");
        let candidates = experimental_typo_correction(
            &mut model,
            "",
            &composing,
            &InputStyle::Direct,
            &tables,
            &LmTypoConfig::default(),
        )
        .unwrap();
        assert_eq!(candidates[0].corrected_input, "キ");
        assert_eq!(candidates[0].converted_text, "キ");
        assert_eq!(candidates[0].channel_cost, 2.0);
        assert_eq!(candidates[0].prominence, 1.0);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.corrected_input == "カ")
        );
    }

    #[test]
    fn roman_input_can_recover_an_adjacent_transposition() {
        let (composing, tables) = composing("ak", InputStyle::RomanToKana);
        let mut model = TestModel::new(&["\u{ee00}", "カ", "ア", "k", "a"], "カ");
        let config = LmTypoConfig {
            beam_size: 128,
            n_best: 64,
            transposition_cost: 0.0,
            ..LmTypoConfig::default()
        };
        let candidates = experimental_typo_correction(
            &mut model,
            "",
            &composing,
            &InputStyle::RomanToKana,
            &tables,
            &config,
        )
        .unwrap();
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.corrected_input == "ka")
        );
    }

    #[test]
    fn deletion_requires_the_observed_key_to_neighbor_the_previous_key() {
        let (composing, tables) = composing("sda", InputStyle::RomanToKana);
        let mut model = TestModel::new(&["\u{ee00}", "サ", "ダ", "s", "d", "a"], "サ");
        let config = LmTypoConfig {
            beam_size: 128,
            n_best: 64,
            deletion_cost: 0.0,
            ..LmTypoConfig::default()
        };
        let candidates = experimental_typo_correction(
            &mut model,
            "",
            &composing,
            &InputStyle::RomanToKana,
            &tables,
            &config,
        )
        .unwrap();
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.corrected_input == "sa")
        );
        assert!(
            !candidates
                .iter()
                .any(|candidate| candidate.corrected_input == "sd")
        );
    }

    #[test]
    fn max_steps_completes_the_unexplored_suffix_as_observed_input() {
        let (composing, tables) = composing("kana", InputStyle::RomanToKana);
        let mut model = TestModel::new(&["\u{ee00}", "カ", "ナ", "k", "a", "n"], "カ");
        let config = LmTypoConfig {
            max_steps: Some(0),
            ..LmTypoConfig::default()
        };
        let candidates = experimental_typo_correction(
            &mut model,
            "",
            &composing,
            &InputStyle::RomanToKana,
            &tables,
            &config,
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].corrected_input, "kana");
        assert_eq!(candidates[0].channel_cost, 0.0);
    }

    #[test]
    fn zero_sized_limits_are_clamped_like_the_upstream_configuration() {
        let (composing, tables) = composing("か", InputStyle::Direct);
        let mut model = TestModel::new(&["\u{ee00}", "カ", "キ", "ク", "ケ", "コ"], "カ");
        let candidates = experimental_typo_correction(
            &mut model,
            "",
            &composing,
            &InputStyle::Direct,
            &tables,
            &LmTypoConfig {
                beam_size: 0,
                top_k: 0,
                n_best: 0,
                ..LmTypoConfig::default()
            },
        )
        .unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].corrected_input, "カ");
    }
}
