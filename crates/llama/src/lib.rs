use std::error::Error;
use std::ffi::{CString, c_char, c_int};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Once;

use llama_cpp_sys_2 as sys;

pub const CONTEXT_TOKEN_LIMIT: usize = 512;

static BACKEND_INIT: Once = Once::new();

struct Model(NonNull<sys::llama_model>);

impl Drop for Model {
    fn drop(&mut self) {
        // SAFETY: This model is uniquely owned and outlives its inference context.
        unsafe { sys::llama_model_free(self.0.as_ptr()) };
    }
}

struct Context(NonNull<sys::llama_context>);

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: This context is uniquely owned and freed before its model.
        unsafe { sys::llama_free(self.0.as_ptr()) };
    }
}

struct Batch(sys::llama_batch);

impl Drop for Batch {
    fn drop(&mut self) {
        // SAFETY: The batch was allocated by llama_batch_init and is freed exactly once.
        unsafe { sys::llama_batch_free(self.0) };
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum LlamaError {
    InvalidPath,
    Load(String),
    InputTooLong(usize),
    InvalidLogitsRange {
        start_index: usize,
        token_count: usize,
    },
    Tokenization,
    TokenPiece(c_int),
    Decode(c_int),
}

impl fmt::Display for LlamaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => write!(formatter, "model path cannot be represented by llama.cpp"),
            Self::Load(message) => write!(formatter, "model loading failed: {message}"),
            Self::InputTooLong(count) => {
                write!(
                    formatter,
                    "inference requires {count} tokens, exceeding {CONTEXT_TOKEN_LIMIT}"
                )
            }
            Self::InvalidLogitsRange {
                start_index,
                token_count,
            } => write!(
                formatter,
                "logits start index {start_index} is outside {token_count} input tokens"
            ),
            Self::Tokenization => write!(formatter, "llama.cpp tokenization failed"),
            Self::TokenPiece(token) => write!(formatter, "could not decode token {token}"),
            Self::Decode(code) => write!(formatter, "llama.cpp decode failed with code {code}"),
        }
    }
}

impl Error for LlamaError {}

pub struct LlamaContext {
    // Fields are dropped in declaration order: batch, context, then its model.
    batch: Batch,
    context: Context,
    _model: Model,
    vocabulary: NonNull<sys::llama_vocab>,
    vocabulary_size: usize,
    sequence_tokens: [Vec<i32>; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LlamaSequence {
    Evaluation,
    InputPrediction,
}

impl LlamaSequence {
    const fn index(self) -> usize {
        match self {
            Self::Evaluation => 0,
            Self::InputPrediction => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SequencePlan {
    source_sequence_id: i32,
    cached_prefix_count: usize,
}

// SAFETY: LlamaContext uniquely owns its model, context and batch. Moving that ownership to another
// thread is safe; its mutable inference methods still require exclusive Rust access.
unsafe impl Send for LlamaContext {}

impl LlamaContext {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LlamaError> {
        let path = model_path(path.as_ref())?;
        BACKEND_INIT.call_once(|| {
            // SAFETY: Backend initialization is process-wide and runs exactly once. Backends remain
            // initialized while any model/context may exist; no context frees the global backend.
            unsafe { sys::llama_backend_init() };
        });

        // SAFETY: The default parameters contain valid values supplied by this exact sys binding.
        let mut model_params = unsafe { sys::llama_model_default_params() };
        // SAFETY: This C record contains only integer tags, byte arrays and a scalar/byte-array union.
        // A zeroed key terminates the override list, matching llama.cpp's API contract.
        let mut overrides: [sys::llama_model_kv_override; 2] = unsafe { std::mem::zeroed() };
        overrides[0].tag = sys::LLAMA_KV_OVERRIDE_TYPE_STR;
        // SAFETY: Both constant strings, including their terminators, fit their 128-byte arrays.
        unsafe {
            std::ptr::copy_nonoverlapping(
                c"tokenizer.ggml.pre".as_ptr(),
                overrides[0].key.as_mut_ptr(),
                c"tokenizer.ggml.pre".to_bytes_with_nul().len(),
            );
            std::ptr::copy_nonoverlapping(
                c"gpt-2".as_ptr(),
                overrides[0].__bindgen_anon_1.val_str.as_mut_ptr(),
                c"gpt-2".to_bytes_with_nul().len(),
            );
        }
        model_params.kv_overrides = overrides.as_ptr();
        // Keep the default GPU offload policy: macOS uses the bundled Metal backend.
        // SAFETY: The path and sentinel-terminated override array remain valid through model loading.
        let model = Model(
            NonNull::new(unsafe { sys::llama_model_load_from_file(path.as_ptr(), model_params) })
                .ok_or_else(|| LlamaError::Load("llama.cpp could not load the model".into()))?,
        );
        // SAFETY: The live model owns its vocabulary. It is only accessed immutably through C APIs.
        let vocabulary =
            NonNull::new(unsafe { sys::llama_model_get_vocab(model.0.as_ptr()) }.cast_mut())
                .ok_or_else(|| LlamaError::Load("model has no vocabulary".into()))?;
        // SAFETY: The vocabulary remains valid until the model is dropped.
        let vocabulary_size = unsafe { sys::llama_vocab_n_tokens(vocabulary.as_ptr()) };
        let vocabulary_size = usize::try_from(vocabulary_size)
            .ok()
            .filter(|size| *size > 0)
            .ok_or_else(|| LlamaError::Load("invalid vocabulary size".into()))?;

        let threads = inference_thread_count(
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        )
        .min(c_int::MAX as usize) as c_int;
        // SAFETY: The default parameters are valid for this exact bundled llama.cpp revision.
        let mut context_params = unsafe { sys::llama_context_default_params() };
        context_params.n_ctx = CONTEXT_TOKEN_LIMIT as u32;
        context_params.n_batch = CONTEXT_TOKEN_LIMIT as u32;
        context_params.n_ubatch = 64;
        context_params.n_seq_max = 2;
        context_params.kv_unified = true;
        context_params.flash_attn_type = sys::LLAMA_FLASH_ATTN_TYPE_ENABLED;
        context_params.n_threads = threads;
        context_params.n_threads_batch = threads;
        context_params.no_perf = true;
        // SAFETY: The model remains owned until after the context is freed.
        let context = Context(
            NonNull::new(unsafe { sys::llama_init_from_model(model.0.as_ptr(), context_params) })
                .ok_or_else(|| {
                LlamaError::Load("llama.cpp could not create the inference context".into())
            })?,
        );
        // SAFETY: The batch owns token/position/logit buffers for the fixed context limit, with one
        // sequence ID per token. It is freed exactly once by Batch::drop.
        let batch = Batch(unsafe { sys::llama_batch_init(CONTEXT_TOKEN_LIMIT as c_int, 0, 1) });
        Ok(Self {
            batch,
            context,
            _model: model,
            vocabulary,
            vocabulary_size,
            sequence_tokens: std::array::from_fn(|_| Vec::new()),
        })
    }

    pub fn vocabulary_size(&self) -> usize {
        self.vocabulary_size
    }

    pub fn eos_token(&self) -> i32 {
        // SAFETY: The model owns this vocabulary for the lifetime of self.
        unsafe { sys::llama_vocab_eos(self.vocabulary.as_ptr()) }
    }

    pub fn tokenize(&self, text: &str, add_special: bool) -> Result<Vec<i32>, LlamaError> {
        let text_length = c_int::try_from(text.len()).map_err(|_| LlamaError::Tokenization)?;
        let initial_capacity = text.len().saturating_add(2).max(1);
        let mut tokens = vec![0; initial_capacity];
        // SAFETY: All buffers remain valid and their exact lengths are supplied.
        let mut count = unsafe {
            sys::llama_tokenize(
                self.vocabulary.as_ptr(),
                text.as_ptr().cast(),
                text_length,
                tokens.as_mut_ptr(),
                c_int::try_from(tokens.len()).map_err(|_| LlamaError::Tokenization)?,
                add_special,
                false,
            )
        };
        if count < 0 {
            let required = count.checked_neg().ok_or(LlamaError::Tokenization)?;
            tokens.resize(
                usize::try_from(required).map_err(|_| LlamaError::Tokenization)?,
                0,
            );
            // SAFETY: The resized token buffer has the capacity requested by llama.cpp.
            count = unsafe {
                sys::llama_tokenize(
                    self.vocabulary.as_ptr(),
                    text.as_ptr().cast(),
                    text_length,
                    tokens.as_mut_ptr(),
                    required,
                    add_special,
                    false,
                )
            };
        }
        let count = usize::try_from(count).map_err(|_| LlamaError::Tokenization)?;
        tokens.truncate(count);
        Ok(tokens)
    }

    pub fn token_to_piece(&self, token: i32) -> Result<Vec<u8>, LlamaError> {
        if token < 0 || token as usize >= self.vocabulary_size {
            return Err(LlamaError::TokenPiece(token));
        }
        let mut buffer = vec![0 as c_char; 8];
        // SAFETY: The writable buffer remains valid for this call.
        let mut count = unsafe {
            sys::llama_token_to_piece(
                self.vocabulary.as_ptr(),
                token,
                buffer.as_mut_ptr(),
                c_int::try_from(buffer.len()).expect("small fixed buffer"),
                0,
                false,
            )
        };
        if count < 0 {
            let required = count.checked_neg().ok_or(LlamaError::TokenPiece(token))?;
            buffer.resize(
                usize::try_from(required).map_err(|_| LlamaError::TokenPiece(token))?,
                0,
            );
            // SAFETY: The resized piece buffer has the capacity requested by llama.cpp.
            count = unsafe {
                sys::llama_token_to_piece(
                    self.vocabulary.as_ptr(),
                    token,
                    buffer.as_mut_ptr(),
                    required,
                    0,
                    false,
                )
            };
        }
        let count = usize::try_from(count).map_err(|_| LlamaError::TokenPiece(token))?;
        Ok(buffer[..count]
            .iter()
            .map(|byte| byte.to_ne_bytes()[0])
            .collect())
    }

    pub fn next_logits(&mut self, tokens: &[i32]) -> Result<Vec<f32>, LlamaError> {
        let start_index = tokens
            .len()
            .checked_sub(1)
            .ok_or(LlamaError::InvalidLogitsRange {
                start_index: 0,
                token_count: 0,
            })?;
        self.logits(tokens, start_index, LlamaSequence::InputPrediction)
    }

    pub fn logits(
        &mut self,
        tokens: &[i32],
        start_index: usize,
        sequence: LlamaSequence,
    ) -> Result<Vec<f32>, LlamaError> {
        if tokens.len() > CONTEXT_TOKEN_LIMIT {
            return Err(LlamaError::InputTooLong(tokens.len()));
        }
        if start_index >= tokens.len() {
            return Err(LlamaError::InvalidLogitsRange {
                start_index,
                token_count: tokens.len(),
            });
        }
        let row_count = tokens.len() - start_index;
        let logits_count = row_count
            .checked_mul(self.vocabulary_size)
            .ok_or(LlamaError::Decode(-1))?;
        let mut logits = vec![0.0; logits_count];
        let plan = self.sequence_plan(tokens, start_index, sequence);
        let sequence_id = sequence.index() as i32;
        let cached_prefix_count = plan.cached_prefix_count as i32;
        // SAFETY: This live decoder context owns its KV memory.
        let memory = NonNull::new(unsafe { sys::llama_get_memory(self.context.0.as_ptr()) })
            .ok_or(LlamaError::Decode(-1))?;
        // SAFETY: The context uniquely owns its memory and batch. Token positions and batch indices
        // are bounded by CONTEXT_TOKEN_LIMIT; each allocated batch row holds one sequence ID. The
        // destination logit slice has exactly the requested rows times the validated vocabulary size.
        let result = unsafe {
            if plan.source_sequence_id >= 0 {
                sys::llama_memory_seq_rm(memory.as_ptr(), sequence_id, 0, -1);
                sys::llama_memory_seq_cp(
                    memory.as_ptr(),
                    plan.source_sequence_id,
                    sequence_id,
                    0,
                    cached_prefix_count,
                );
            }
            if !sys::llama_memory_seq_rm(memory.as_ptr(), sequence_id, cached_prefix_count, -1) {
                -1
            } else {
                let batch = &mut self.batch.0;
                batch.n_tokens = 0;
                for (index, token) in tokens.iter().enumerate().skip(plan.cached_prefix_count) {
                    let batch_index = batch.n_tokens as usize;
                    *batch.token.add(batch_index) = *token;
                    *batch.pos.add(batch_index) = index as i32;
                    *batch.n_seq_id.add(batch_index) = 1;
                    **batch.seq_id.add(batch_index) = sequence_id;
                    *batch.logits.add(batch_index) = i8::from(index >= start_index);
                    batch.n_tokens += 1;
                }
                let decode_result = sys::llama_decode(self.context.0.as_ptr(), *batch);
                if decode_result != 0 {
                    decode_result
                } else {
                    let model_logits = sys::llama_get_logits(self.context.0.as_ptr());
                    if model_logits.is_null() {
                        -2
                    } else {
                        // llama_get_logits stores every requested output row contiguously.
                        std::ptr::copy_nonoverlapping(
                            model_logits,
                            logits.as_mut_ptr(),
                            logits.len(),
                        );
                        0
                    }
                }
            }
        };
        if result != 0 {
            // SAFETY: Whole-sequence removal always succeeds, including after a partially decoded
            // batch. The other sequence and its shared prefix remain owned by the context.
            unsafe { sys::llama_memory_seq_rm(memory.as_ptr(), sequence_id, 0, -1) };
            self.sequence_tokens[sequence.index()].clear();
            return Err(LlamaError::Decode(result));
        }
        self.sequence_tokens[sequence.index()].clear();
        self.sequence_tokens[sequence.index()].extend_from_slice(tokens);
        Ok(logits)
    }

    fn sequence_plan(
        &self,
        tokens: &[i32],
        start_index: usize,
        sequence: LlamaSequence,
    ) -> SequencePlan {
        let sequence_index = sequence.index();
        let current_prefix = common_prefix_count(&self.sequence_tokens[sequence_index], tokens);
        let other_index = 1 - sequence_index;
        let other_prefix = common_prefix_count(&self.sequence_tokens[other_index], tokens);
        if other_prefix > current_prefix {
            let copied_prefix = other_prefix.min(start_index);
            if copied_prefix > 0 {
                return SequencePlan {
                    source_sequence_id: i32::try_from(other_index).expect("sequence ID fits i32"),
                    cached_prefix_count: copied_prefix,
                };
            }
        }
        SequencePlan {
            source_sequence_id: -1,
            cached_prefix_count: current_prefix.min(start_index),
        }
    }
}

fn common_prefix_count(left: &[i32], right: &[i32]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

fn inference_thread_count(available: usize) -> usize {
    let available = available.max(1);
    let reserved = if available >= 8 {
        2
    } else if available >= 4 {
        1
    } else {
        0
    };
    available.saturating_sub(reserved).clamp(1, 8)
}

fn model_path(path: &Path) -> Result<CString, LlamaError> {
    // llama.cpp opens Windows paths as UTF-8, not Rust's potentially non-Unicode OS encoding.
    #[cfg(windows)]
    let bytes = path.to_str().ok_or(LlamaError::InvalidPath)?.as_bytes();
    #[cfg(not(windows))]
    let bytes = path.as_os_str().as_encoded_bytes();
    CString::new(bytes).map_err(|_| LlamaError::InvalidPath)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_capacity_for_interactive_work() {
        assert_eq!(inference_thread_count(1), 1);
        assert_eq!(inference_thread_count(3), 3);
        assert_eq!(inference_thread_count(6), 5);
        assert_eq!(inference_thread_count(8), 6);
        assert_eq!(inference_thread_count(32), 8);
    }

    #[cfg(unix)]
    #[test]
    fn model_paths_preserve_native_byte_encoding() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let bytes = b"model-\xff.gguf";
        let path = OsString::from_vec(bytes.to_vec());
        assert_eq!(model_path(Path::new(&path)).unwrap().to_bytes(), bytes);
    }

    #[test]
    fn counts_only_the_shared_token_prefix() {
        assert_eq!(common_prefix_count(&[1, 2, 3], &[1, 2, 4]), 2);
        assert_eq!(common_prefix_count(&[1, 2], &[1, 2, 3]), 2);
        assert_eq!(common_prefix_count(&[1], &[2]), 0);
    }
}
