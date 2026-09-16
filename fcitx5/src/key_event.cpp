#include "key_event.h"

#include <fcitx-utils/key.h>
#include <fcitx-utils/keysym.h>
#include <fcitx/event.h>

#include <cstdint>
#include <string>

#include "beankey.pb.h"

namespace beankey {
namespace {

std::string printableKeyText(fcitx::KeySym symbol) {
  const std::uint32_t codePoint = fcitx::Key::keySymToUnicode(symbol);
  if (codePoint < 0x20 || (codePoint >= 0x7f && codePoint <= 0x9f)) {
    return {};
  }
  return fcitx::Key::keySymToUTF8(symbol);
}

std::string japaneseInputText(const std::string &text) {
  return text == "-" ? "ー" : text;
}

v1::UserAction shortcutAction(fcitx::KeySym symbol) {
  switch (symbol) {
  case FcitxKey_h:
    return v1::USER_ACTION_BACKSPACE;
  case FcitxKey_p:
    return v1::USER_ACTION_UP;
  case FcitxKey_m:
    return v1::USER_ACTION_ENTER;
  case FcitxKey_n:
    return v1::USER_ACTION_DOWN;
  case FcitxKey_f:
    return v1::USER_ACTION_RIGHT;
  case FcitxKey_i:
    return v1::USER_ACTION_LEFT;
  case FcitxKey_o:
    return v1::USER_ACTION_RIGHT;
  case FcitxKey_l:
    return v1::USER_ACTION_FULL_WIDTH_ROMAN;
  case FcitxKey_j:
    return v1::USER_ACTION_HIRAGANA;
  case FcitxKey_k:
    return v1::USER_ACTION_KATAKANA;
  case FcitxKey_semicolon:
    return v1::USER_ACTION_HALF_WIDTH_KATAKANA;
  case FcitxKey_colon:
  case FcitxKey_apostrophe:
    return v1::USER_ACTION_HALF_WIDTH_ROMAN;
  default:
    return v1::USER_ACTION_UNSPECIFIED;
  }
}

v1::UserAction keyAction(const fcitx::KeyEvent &source) {
  const auto symbol = source.key().sym();
  const auto states = source.rawKey().states();
  if (states.test(fcitx::KeyState::Ctrl)) {
    const auto logicalSymbol = source.rawKey().sym();
    if (logicalSymbol == FcitxKey_u && states.test(fcitx::KeyState::Shift)) {
      return v1::USER_ACTION_START_UNICODE_INPUT;
    }
    if (logicalSymbol == FcitxKey_Delete ||
        logicalSymbol == FcitxKey_BackSpace) {
      return v1::USER_ACTION_FORGET;
    }
    const auto action = shortcutAction(logicalSymbol);
    return action == v1::USER_ACTION_UNSPECIFIED ? v1::USER_ACTION_CONSUME
                                                 : action;
  }
  switch (symbol) {
  case FcitxKey_BackSpace:
    return v1::USER_ACTION_BACKSPACE;
  case FcitxKey_Return:
  case FcitxKey_KP_Enter:
    return v1::USER_ACTION_ENTER;
  case FcitxKey_Escape:
    return v1::USER_ACTION_ESCAPE;
  case FcitxKey_space:
    return v1::USER_ACTION_SPACE;
  case FcitxKey_Tab:
  case FcitxKey_ISO_Left_Tab:
    return v1::USER_ACTION_TAB;
  case FcitxKey_Left:
    return v1::USER_ACTION_LEFT;
  case FcitxKey_Right:
    return v1::USER_ACTION_RIGHT;
  case FcitxKey_Up:
    return v1::USER_ACTION_UP;
  case FcitxKey_Down:
    return v1::USER_ACTION_DOWN;
  case FcitxKey_Eisu_toggle:
    return v1::USER_ACTION_EISU;
  case FcitxKey_Hiragana_Katakana:
  case FcitxKey_Kana_Lock:
    return v1::USER_ACTION_KANA;
  case FcitxKey_F6:
    return v1::USER_ACTION_HIRAGANA;
  case FcitxKey_F7:
    return v1::USER_ACTION_KATAKANA;
  case FcitxKey_F8:
    return v1::USER_ACTION_HALF_WIDTH_KATAKANA;
  case FcitxKey_F9:
    return v1::USER_ACTION_FULL_WIDTH_ROMAN;
  case FcitxKey_F10:
    return v1::USER_ACTION_HALF_WIDTH_ROMAN;
  default:
    return printableKeyText(symbol).empty() ? v1::USER_ACTION_UNSPECIFIED
                                            : v1::USER_ACTION_INPUT;
  }
}

} // namespace

void populateKeyEvent(const fcitx::KeyEvent &source, v1::KeyEvent *target) {
  target->set_action(keyAction(source));
  const auto states = source.rawKey().states();
  if (states.test(fcitx::KeyState::Shift)) {
    target->set_shift(true);
  }
  if (states.test(fcitx::KeyState::Ctrl) &&
      (source.rawKey().sym() == FcitxKey_i ||
       source.rawKey().sym() == FcitxKey_o)) {
    target->set_shift(true);
  }
  const fcitx::KeyStates shortcutModifiers(
      {fcitx::KeyState::Ctrl, fcitx::KeyState::Alt, fcitx::KeyState::Super,
       fcitx::KeyState::Super2, fcitx::KeyState::Hyper, fcitx::KeyState::Meta});
  const fcitx::KeyStates nonOptionModifiers(
      {fcitx::KeyState::Ctrl, fcitx::KeyState::Super, fcitx::KeyState::Super2,
       fcitx::KeyState::Hyper, fcitx::KeyState::Meta});
  target->set_option(states.test(fcitx::KeyState::Alt) &&
                     !states.testAny(nonOptionModifiers));
  if (target->action() == v1::USER_ACTION_INPUT &&
      !states.testAny(shortcutModifiers)) {
    target->set_text(japaneseInputText(printableKeyText(source.key().sym())));
  }
  const std::string inputText = printableKeyText(source.key().sym());
  const std::string rawText = printableKeyText(source.rawKey().sym());
  target->set_input(target->option() && !target->shift()
                        ? rawText
                        : (inputText.empty() ? rawText : inputText));
  target->set_intention(japaneseInputText(rawText));
}

} // namespace beankey
