use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;

use beankey_converter::{HunspellCompleter, HunspellError};
use tempfile::TempDir;

const AFFIX: &[u8] = b"SET UTF-8\nTRY abcdefghijklmnopqrstuvwxyz\n";
const DICTIONARY: &[u8] = b"2\nhell\nhelp\n";

fn fixture(affix: &[u8], dictionary: &[u8]) -> TempDir {
    let directory = TempDir::new().unwrap();
    fs::write(directory.path().join("words.aff"), affix).unwrap();
    fs::write(directory.path().join("words.dic"), dictionary).unwrap();
    directory
}

fn open_error(base: &Path) -> HunspellError {
    HunspellCompleter::open(base, base)
        .err()
        .expect("invalid completion dictionary must fail during open")
}

#[test]
fn reports_missing_dictionary_files_with_their_io_error() {
    let directory = TempDir::new().unwrap();
    let base = directory.path().join("words");
    for extension in ["aff", "dic"] {
        let error = open_error(&base);
        assert!(error.source().is_some());
        assert!(matches!(
            error,
            HunspellError::Io { path, source }
                if path == base.with_extension(extension) && source.kind() == io::ErrorKind::NotFound
        ));
        fs::write(base.with_extension("aff"), AFFIX).unwrap();
    }
}

#[test]
fn rejects_missing_and_unsupported_encodings() {
    let directory = fixture(b"TRY abc\n", DICTIONARY);
    let base = directory.path().join("words");
    assert!(matches!(
        open_error(&base),
        HunspellError::MissingEncoding(path) if path == base.with_extension("aff")
    ));

    let directory = fixture(b"SET unsupported-encoding\n", DICTIONARY);
    assert!(matches!(
        open_error(&directory.path().join("words")),
        HunspellError::UnsupportedEncoding(encoding) if encoding == "unsupported-encoding"
    ));
}

#[test]
fn rejects_invalid_bytes_in_either_dictionary_file() {
    for (affix, dictionary, extension) in [
        (b"SET UTF-8\nTRY \xff\n".as_slice(), DICTIONARY, "aff"),
        (AFFIX, b"1\nhel\xff\n".as_slice(), "dic"),
    ] {
        let directory = fixture(affix, dictionary);
        let base = directory.path().join("words");
        assert!(matches!(
            open_error(&base),
            HunspellError::InvalidEncoding { path, encoding }
                if path == base.with_extension(extension) && encoding == "UTF-8"
        ));
    }
}

#[test]
fn rejects_malformed_or_empty_dictionaries() {
    for (affix, dictionary) in [
        (b"SET UTF-8\nPFX A Y invalid\n".as_slice(), DICTIONARY),
        (AFFIX, b"not-a-count\nhello\n".as_slice()),
        (AFFIX, b"".as_slice()),
        (AFFIX, b"0\n".as_slice()),
    ] {
        let directory = fixture(affix, dictionary);
        let base = directory.path().join("words");
        assert!(matches!(
            open_error(&base),
            HunspellError::InvalidDictionary { affix, dictionary, message }
                if affix == base.with_extension("aff")
                    && dictionary == base.with_extension("dic")
                    && !message.is_empty()
        ));
    }
}

#[test]
fn fails_open_when_the_second_language_dictionary_is_invalid() {
    let english = fixture(AFFIX, DICTIONARY);
    let greek = fixture(b"SET unsupported-encoding\n", DICTIONARY);
    let error = HunspellCompleter::open(english.path().join("words"), greek.path().join("words"))
        .err()
        .expect("both language dictionaries must load before open succeeds");
    assert!(matches!(error, HunspellError::UnsupportedEncoding(_)));
}
