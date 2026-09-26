{
  cfg,
  lib,
  pkgs,
  packages,
}:
let
  inherit (lib)
    mkEnableOption
    mkOption
    optionalAttrs
    types
    ;
  predictionType = types.enum [
    "automatic"
    "manual"
    "disabled"
  ];
  inputStyleType = types.enum [
    "direct"
    "roman_to_kana"
    "azik"
    "kana_jis"
    "kana_us"
    "custom"
  ];
  optionalString =
    description:
    mkOption {
      type = types.nullOr types.str;
      default = null;
      inherit description;
    };
  absolutePath =
    description:
    mkOption {
      type = types.nullOr types.str;
      default = null;
      inherit description;
    };

  personalizationConfig = types.submodule {
    options = {
      baseNgram = mkOption {
        type = types.str;
        description = "Absolute prefix for the base EfficientNGram files.";
      };
      personalNgram = mkOption {
        type = types.str;
        description = "Absolute prefix for the personal EfficientNGram files.";
      };
      alpha = mkOption {
        type = types.float;
        default = 1.0;
        description = "Interpolation strength for the personal language model.";
      };
    };
  };

  typoNgramConfig = types.submodule {
    options = {
      prefix = mkOption {
        type = types.str;
        description = "Absolute prefix for the EfficientNGram files.";
      };
      n = mkOption {
        type = types.ints.positive;
        default = 5;
        description = "N-gram order.";
      };
      discount = mkOption {
        type = types.float;
        default = 0.75;
        description = "N-gram discount value.";
      };
    };
  };

  conversionConfig = {
    input_style = cfg.conversion.inputStyle;
    keyboard_language = cfg.conversion.keyboardLanguage;
    n_best = cfg.conversion.candidateCount;
    japanese_prediction = cfg.conversion.japanesePrediction;
    foreign_prediction = cfg.conversion.foreignPrediction;
    full_width_roman = cfg.conversion.fullWidthRoman;
    half_width_kana = cfg.conversion.halfWidthKana;
    typography = cfg.conversion.typography;
    typo_correction = cfg.conversion.typoCorrection;
    live_conversion = cfg.conversion.liveConversion;
    type_backslash = cfg.conversion.typeBackslash;
    type_half_space = cfg.conversion.typeHalfSpace;
    option_direct_full_width_input = cfg.conversion.optionDirectFullWidthInput;
    punctuation_style = cfg.conversion.punctuationStyle;
    custom_input_tables = lib.mapAttrs (_: value: toString value) cfg.conversion.customInputTables;
  }
  // optionalAttrs (cfg.conversion.customInputTable != null) {
    custom_input_table = cfg.conversion.customInputTable;
  }
  // optionalAttrs (cfg.conversion.userDictionary != null) {
    user_dictionary = cfg.conversion.userDictionary;
  }
  // optionalAttrs (cfg.conversion.userDictionaryDirectory != null) {
    user_dictionary_directory = cfg.conversion.userDictionaryDirectory;
  };

  zenzConfig = {
    inference_limit = cfg.zenz.inferenceLimit;
    rich_candidates = cfg.zenz.richCandidates;
    predictive_input = cfg.zenz.predictiveInput;
    enable_alignment_separator = cfg.zenz.enableAlignmentSeparator;
  }
  // optionalAttrs (cfg.zenz.profile != null) { inherit (cfg.zenz) profile; }
  // optionalAttrs (cfg.zenz.topic != null) { inherit (cfg.zenz) topic; }
  // optionalAttrs (cfg.zenz.style != null) { inherit (cfg.zenz) style; }
  // optionalAttrs (cfg.zenz.preference != null) { inherit (cfg.zenz) preference; }
  // optionalAttrs (cfg.zenz.personalization != null) {
    personalization = {
      base_ngram = cfg.zenz.personalization.baseNgram;
      personal_ngram = cfg.zenz.personalization.personalNgram;
      inherit (cfg.zenz.personalization) alpha;
    };
  };

  lmTypoConfig = {
    inherit (cfg.lmTypo) enabled;
    language_model = cfg.lmTypo.languageModel;
    beam_size = cfg.lmTypo.beamSize;
    top_k = cfg.lmTypo.topK;
    n_best = cfg.lmTypo.candidateCount;
    substitution_cost = cfg.lmTypo.substitutionCost;
    deletion_cost = cfg.lmTypo.deletionCost;
    transposition_cost = cfg.lmTypo.transpositionCost;
  }
  // optionalAttrs (cfg.lmTypo.maxSteps != null) {
    max_steps = cfg.lmTypo.maxSteps;
  }
  // optionalAttrs (cfg.lmTypo.ngram != null) {
    ngram = {
      inherit (cfg.lmTypo.ngram) discount n;
      prefix = cfg.lmTypo.ngram.prefix;
      tokenizer = "${packages.tokenizer}/share/beankey/tokenizer/tokenizer.json";
    };
  };

  configFile = (pkgs.formats.toml { }).generate "beankey-config.toml" {
    dictionary = "${packages.dictionary}/share/beankey/dictionary";
    model = "${packages.model}/share/beankey/model/ggml-model-Q5_K_M.gguf";
    emoji_dictionary = "${packages.emoji}/share/beankey/emoji/emoji_all_E17.0.txt";
    llama_backend_directory = "${packages.daemon.llamaCpp}/bin";
    runtime_socket = "beankey/daemon.sock";
    hunspell = {
      english_dictionary = "${packages.daemon.hunspellEnglish}/share/hunspell/en_US";
      greek_dictionary = "${packages.daemon.hunspellGreek}/share/hunspell/el_GR";
    };
    conversion = conversionConfig;
    learning = {
      mode = cfg.learning.mode;
      max_count = cfg.learning.maxCount;
    };
    zenz = zenzConfig;
    lm_typo = lmTypoConfig;
    inference = {
      context_size = 512;
      batch_size = 512;
      micro_batch_size = 64;
      flash_attention = true;
    };
  };
in
{
  inherit configFile;
  options = {
    enable = mkEnableOption "beanKey kana-kanji conversion";

    useBeanKeyTheme = mkOption {
      type = types.bool;
      default = false;
      description = "Whether to apply the beanKey Fcitx5 Classic UI theme on NixOS. Ignored on macOS.";
    };

    conversion = {
      inputStyle = mkOption {
        type = inputStyleType;
        default = "roman_to_kana";
        description = "Default input style.";
      };
      customInputTable = optionalString "Name of the default custom input table.";
      keyboardLanguage = mkOption {
        type = types.enum [
          "none"
          "japanese"
          "english_us"
          "greek"
        ];
        default = "japanese";
        description = "Keyboard language used for foreign word completion.";
      };
      candidateCount = mkOption {
        type = types.ints.positive;
        default = 10;
        description = "Number of conversion candidates to request.";
      };
      japanesePrediction = mkOption {
        type = predictionType;
        default = "disabled";
        description = "Japanese prediction mode.";
      };
      foreignPrediction = mkOption {
        type = predictionType;
        default = "disabled";
        description = "English and Greek prediction mode.";
      };
      fullWidthRoman = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to generate full-width Roman candidates.";
      };
      halfWidthKana = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to generate half-width Kana candidates.";
      };
      typography = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to generate typography variants.";
      };
      typoCorrection = mkOption {
        type = types.enum [
          "enabled"
          "automatic"
          "disabled"
        ];
        default = "automatic";
        description = "Dictionary typo correction mode.";
      };
      liveConversion = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to display the best exact candidate while composing.";
      };
      typeBackslash = mkOption {
        type = types.bool;
        default = false;
        description = "Whether the Yen key inputs a backslash instead of a Yen sign.";
      };
      typeHalfSpace = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Space inputs a half-width space and Shift-Space inputs a full-width space.";
      };
      optionDirectFullWidthInput = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Alt (Option) directly inputs full-width alphanumeric text when no composition is active.";
      };
      punctuationStyle = mkOption {
        type = types.enum [
          "kuten_and_toten"
          "kuten_and_comma"
          "period_and_toten"
          "period_and_comma"
        ];
        default = "kuten_and_toten";
        description = "Punctuation pair used for the comma and period keys.";
      };
      userDictionary = absolutePath "Absolute path to a JSON user dictionary.";
      userDictionaryDirectory = absolutePath "Absolute path to an azooKey user dictionary directory.";
      customInputTables = mkOption {
        type = types.attrsOf types.path;
        default = { };
        description = "Named custom input table JSON files.";
      };
    };

    learning = {
      mode = mkOption {
        type = types.enum [
          "input_and_output"
          "only_output"
          "nothing"
        ];
        default = "input_and_output";
        description = "Whether learning data is updated, only read, or ignored.";
      };
      maxCount = mkOption {
        type = types.ints.unsigned;
        default = 65536;
        description = "Maximum number of persisted learning records.";
      };
    };

    zenz = {
      inferenceLimit = mkOption {
        type = types.ints.between 1 50;
        default = 5;
        description = "Maximum Zenz prefix correction attempts, between 1 and 50.";
      };
      richCandidates = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to request rich Zenz candidates.";
      };
      predictiveInput = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to enable Zenz next-input prediction.";
      };
      profile = optionalString "Zenz user profile condition.";
      topic = optionalString "Zenz topic condition.";
      style = optionalString "Zenz style condition.";
      preference = optionalString "Zenz preference condition.";
      enableAlignmentSeparator = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to include alignment separators in Zenz prompts.";
      };
      personalization = mkOption {
        type = types.nullOr personalizationConfig;
        default = null;
        description = "Optional EfficientNGram personalization models.";
      };
    };

    lmTypo = {
      enabled = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to expose experimental LM typo corrections.";
      };
      languageModel = mkOption {
        type = types.enum [
          "zenz"
          "ngram"
        ];
        default = "zenz";
        description = "Language model used by LM typo correction.";
      };
      ngram = mkOption {
        type = types.nullOr typoNgramConfig;
        default = null;
        description = "EfficientNGram model used for LM typo correction.";
      };
      beamSize = mkOption {
        type = types.ints.positive;
        default = 32;
        description = "LM typo correction beam size.";
      };
      topK = mkOption {
        type = types.ints.positive;
        default = 64;
        description = "Token branching limit for LM typo correction.";
      };
      candidateCount = mkOption {
        type = types.ints.positive;
        default = 5;
        description = "Number of LM typo correction candidates.";
      };
      maxSteps = mkOption {
        type = types.nullOr types.ints.positive;
        default = null;
        description = "Optional decoding step limit for LM typo correction.";
      };
      substitutionCost = mkOption {
        type = types.float;
        default = 2.0;
        description = "LM typo substitution channel cost.";
      };
      deletionCost = mkOption {
        type = types.float;
        default = 3.0;
        description = "LM typo deletion channel cost.";
      };
      transpositionCost = mkOption {
        type = types.float;
        default = 2.0;
        description = "LM typo transposition channel cost.";
      };
    };
  };
  assertions = [
    {
      assertion =
        cfg.conversion.inputStyle != "custom"
        || (
          cfg.conversion.customInputTable != null
          && builtins.hasAttr cfg.conversion.customInputTable cfg.conversion.customInputTables
        );
      message = "programs.beanKey.conversion.customInputTable must name a registered table when inputStyle is custom";
    }
    {
      assertion = cfg.zenz.personalization == null || cfg.zenz.personalization.alpha >= 0.0;
      message = "programs.beanKey.zenz.personalization.alpha must be nonnegative";
    }
    {
      assertion = cfg.lmTypo.languageModel != "ngram" || cfg.lmTypo.ngram != null;
      message = "programs.beanKey.lmTypo.ngram is required when languageModel is ngram";
    }
    {
      assertion = builtins.all (value: value == null || lib.hasPrefix "/" value) [
        cfg.conversion.userDictionary
        cfg.conversion.userDictionaryDirectory
        (if cfg.zenz.personalization == null then null else cfg.zenz.personalization.baseNgram)
        (if cfg.zenz.personalization == null then null else cfg.zenz.personalization.personalNgram)
        (if cfg.lmTypo.ngram == null then null else cfg.lmTypo.ngram.prefix)
      ];
      message = "programs.beanKey runtime data paths must be absolute";
    }
  ];
}
