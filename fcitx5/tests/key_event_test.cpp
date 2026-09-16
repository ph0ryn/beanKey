#include "key_event.h"

#include <fcitx-utils/key.h>
#include <fcitx-utils/keysym.h>
#include <fcitx/event.h>

#include <array>
#include <cstdint>
#include <iostream>
#include <string_view>
#include <utility>

#include "beankey.pb.h"

namespace {

struct KeyCase {
  std::string_view name;
  fcitx::KeySym symbol;
};

bool rejectsControlText(const KeyCase &keyCase) {
  fcitx::KeyEvent source(nullptr, fcitx::Key(keyCase.symbol));
  beankey::v1::KeyEvent target;
  beankey::populateKeyEvent(source, &target);

  if (target.text().empty() && target.input().empty() &&
      target.intention().empty()) {
    return true;
  }
  std::cerr << keyCase.name << " leaked control text into the protocol\n";
  return false;
}

} // namespace

int main() {
  fcitx::KeyEvent printableSource(nullptr, fcitx::Key(FcitxKey_a));
  beankey::v1::KeyEvent printableTarget;
  beankey::populateKeyEvent(printableSource, &printableTarget);
  if (printableTarget.action() != beankey::v1::USER_ACTION_INPUT ||
      printableTarget.text() != "a" || printableTarget.input() != "a" ||
      printableTarget.intention() != "a") {
    std::cerr << "printable key mapping changed unexpectedly\n";
    return 1;
  }

  fcitx::KeyEvent longVowelSource(nullptr, fcitx::Key(FcitxKey_minus));
  beankey::v1::KeyEvent longVowelTarget;
  beankey::populateKeyEvent(longVowelSource, &longVowelTarget);
  if (longVowelTarget.action() != beankey::v1::USER_ACTION_INPUT ||
      longVowelTarget.text() != "ー" || longVowelTarget.input() != "-" ||
      longVowelTarget.intention() != "ー") {
    std::cerr << "Japanese hyphen key did not produce a long vowel mark\n";
    return 1;
  }

  constexpr std::array controlKeys{
      KeyCase{"BackSpace", FcitxKey_BackSpace},
      KeyCase{"Return", FcitxKey_Return},
      KeyCase{"KP_Enter", FcitxKey_KP_Enter},
      KeyCase{"Delete", FcitxKey_Delete},
      KeyCase{"Escape", FcitxKey_Escape},
      KeyCase{"Tab", FcitxKey_Tab},
      KeyCase{"ISO_Left_Tab", FcitxKey_ISO_Left_Tab},
  };
  bool valid = true;
  for (const auto &keyCase : controlKeys) {
    valid = rejectsControlText(keyCase) && valid;
  }
  fcitx::KeyEvent backspaceSource(nullptr, fcitx::Key(FcitxKey_BackSpace));
  beankey::v1::KeyEvent backspaceTarget;
  beankey::populateKeyEvent(backspaceSource, &backspaceTarget);
  if (backspaceTarget.action() != beankey::v1::USER_ACTION_BACKSPACE) {
    std::cerr << "Backspace semantic action was not mapped\n";
    valid = false;
  }

  fcitx::KeyEvent controlHSource(nullptr,
                                 fcitx::Key(FcitxKey_h, fcitx::KeyState::Ctrl));
  beankey::v1::KeyEvent controlHTarget;
  beankey::populateKeyEvent(controlHSource, &controlHTarget);
  if (controlHTarget.action() != beankey::v1::USER_ACTION_BACKSPACE ||
      !controlHTarget.text().empty()) {
    std::cerr << "Control-H semantic action was not mapped\n";
    valid = false;
  }

  for (const auto &[symbol, expected] :
       {std::pair{FcitxKey_j, beankey::v1::USER_ACTION_HIRAGANA},
        std::pair{FcitxKey_semicolon,
                  beankey::v1::USER_ACTION_HALF_WIDTH_KATAKANA}}) {
    fcitx::KeyEvent source(nullptr, fcitx::Key(symbol, fcitx::KeyState::Ctrl));
    beankey::v1::KeyEvent target;
    beankey::populateKeyEvent(source, &target);
    if (target.action() != expected || !target.text().empty()) {
      std::cerr << "known Control shortcut lost its semantic action\n";
      valid = false;
    }
  }

  fcitx::KeyEvent controlDeleteSource(
      nullptr, fcitx::Key(FcitxKey_Delete, fcitx::KeyState::Ctrl));
  beankey::v1::KeyEvent controlDeleteTarget;
  beankey::populateKeyEvent(controlDeleteSource, &controlDeleteTarget);
  if (controlDeleteTarget.action() != beankey::v1::USER_ACTION_FORGET) {
    std::cerr << "Control-Delete did not map to candidate forgetting\n";
    valid = false;
  }

  fcitx::KeyEvent controlQSource(nullptr,
                                 fcitx::Key(FcitxKey_q, fcitx::KeyState::Ctrl));
  beankey::v1::KeyEvent controlQTarget;
  beankey::populateKeyEvent(controlQSource, &controlQTarget);
  if (controlQTarget.action() != beankey::v1::USER_ACTION_CONSUME ||
      !controlQTarget.text().empty()) {
    std::cerr << "Undefined Control shortcut was not marked for consumption\n";
    valid = false;
  }

  fcitx::KeyEvent unicodeSource(
      nullptr,
      fcitx::Key(FcitxKey_u, fcitx::KeyStates{fcitx::KeyState::Ctrl,
                                              fcitx::KeyState::Shift}));
  beankey::v1::KeyEvent unicodeTarget;
  beankey::populateKeyEvent(unicodeSource, &unicodeTarget);
  if (unicodeTarget.action() != beankey::v1::USER_ACTION_START_UNICODE_INPUT) {
    std::cerr << "Control-Shift-U Unicode input was not mapped\n";
    valid = false;
  }

  for (const auto &keyCase : {KeyCase{"Delete", FcitxKey_Delete},
                              KeyCase{"Page Up", FcitxKey_Page_Up},
                              KeyCase{"Page Down", FcitxKey_Page_Down}}) {
    fcitx::KeyEvent source(nullptr, fcitx::Key(keyCase.symbol));
    beankey::v1::KeyEvent target;
    beankey::populateKeyEvent(source, &target);
    if (target.action() != beankey::v1::USER_ACTION_UNSPECIFIED) {
      std::cerr << keyCase.name << " unsupported action was mapped\n";
      valid = false;
    }
  }

  for (const auto &[symbol, expected] :
       {std::pair{FcitxKey_Eisu_toggle, beankey::v1::USER_ACTION_EISU},
        std::pair{FcitxKey_Hiragana_Katakana, beankey::v1::USER_ACTION_KANA},
        std::pair{FcitxKey_Kana_Lock, beankey::v1::USER_ACTION_KANA}}) {
    fcitx::KeyEvent source(nullptr, fcitx::Key(symbol));
    beankey::v1::KeyEvent target;
    beankey::populateKeyEvent(source, &target);
    if (target.action() != expected) {
      std::cerr << "input language key was not mapped\n";
      valid = false;
    }
  }

  for (const auto &[symbol, expected] :
       {std::pair{FcitxKey_F6, beankey::v1::USER_ACTION_HIRAGANA},
        std::pair{FcitxKey_F7, beankey::v1::USER_ACTION_KATAKANA},
        std::pair{FcitxKey_F8, beankey::v1::USER_ACTION_HALF_WIDTH_KATAKANA},
        std::pair{FcitxKey_F9, beankey::v1::USER_ACTION_FULL_WIDTH_ROMAN},
        std::pair{FcitxKey_F10, beankey::v1::USER_ACTION_HALF_WIDTH_ROMAN}}) {
    fcitx::KeyEvent source(nullptr, fcitx::Key(symbol));
    beankey::v1::KeyEvent target;
    beankey::populateKeyEvent(source, &target);
    if (target.action() != expected) {
      std::cerr << "Desktop function key was not mapped\n";
      valid = false;
    }
  }

  for (const auto &[symbol, expected] :
       {std::pair{FcitxKey_Left, beankey::v1::USER_ACTION_LEFT},
        std::pair{FcitxKey_Right, beankey::v1::USER_ACTION_RIGHT}}) {
    fcitx::KeyEvent source(nullptr, fcitx::Key(symbol, fcitx::KeyState::Shift));
    beankey::v1::KeyEvent target;
    beankey::populateKeyEvent(source, &target);
    if (target.action() != expected || !target.shift()) {
      std::cerr << "Shift navigation lost its segment-edit intention\n";
      valid = false;
    }
  }

  fcitx::KeyEvent shiftedSpaceSource(
      nullptr, fcitx::Key(FcitxKey_space, fcitx::KeyState::Shift));
  beankey::v1::KeyEvent shiftedSpaceTarget;
  beankey::populateKeyEvent(shiftedSpaceSource, &shiftedSpaceTarget);
  if (shiftedSpaceTarget.action() != beankey::v1::USER_ACTION_SPACE ||
      !shiftedSpaceTarget.shift()) {
    std::cerr << "Shift-Space semantic action was not mapped\n";
    valid = false;
  }

  fcitx::KeyEvent optionSource(nullptr,
                               fcitx::Key(FcitxKey_a, fcitx::KeyState::Alt));
  beankey::v1::KeyEvent optionTarget;
  beankey::populateKeyEvent(optionSource, &optionTarget);
  if (optionTarget.action() != beankey::v1::USER_ACTION_INPUT ||
      !optionTarget.option() || !optionTarget.text().empty() ||
      optionTarget.input() != "a") {
    std::cerr << "Alt was not mapped to the semantic Option modifier\n";
    valid = false;
  }

  fcitx::KeyEvent shiftedOptionSource(
      nullptr, fcitx::Key(FcitxKey_a, fcitx::KeyStates{fcitx::KeyState::Shift,
                                                       fcitx::KeyState::Alt}));
  beankey::v1::KeyEvent shiftedOptionTarget;
  beankey::populateKeyEvent(shiftedOptionSource, &shiftedOptionTarget);
  if (!shiftedOptionTarget.option() || !shiftedOptionTarget.shift() ||
      shiftedOptionTarget.input() != "A") {
    std::cerr << "Shift-Alt input lost its shifted printable text\n";
    valid = false;
  }

  fcitx::KeyEvent modifiedOptionSource(
      nullptr,
      fcitx::Key(FcitxKey_a, fcitx::KeyStates{fcitx::KeyState::Alt,
                                              fcitx::KeyState::Super}));
  beankey::v1::KeyEvent modifiedOptionTarget;
  beankey::populateKeyEvent(modifiedOptionSource, &modifiedOptionTarget);
  if (modifiedOptionTarget.option()) {
    std::cerr << "Alt with another shortcut modifier became Option\n";
    valid = false;
  }
  return valid ? 0 : 1;
}
