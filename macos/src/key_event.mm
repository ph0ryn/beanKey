#include "key_event.h"
#import <Carbon/Carbon.h>

namespace beankey::macos {
namespace {
std::string printable(NSString *text) {
  if (text.length == 0)
    return {};
  const unichar first = [text characterAtIndex:0];
  if (first < 0x20 || (first >= 0x7f && first <= 0x9f) ||
      (first >= 0xf700 && first <= 0xf8ff))
    return {};
  const char *utf8 = text.UTF8String;
  return utf8 ? utf8 : "";
}
std::string japanese(const std::string &text) {
  return text == "-" ? "ー" : text;
}
v1::UserAction controlAction(NSString *raw, unsigned short code, bool shift) {
  if (code == kVK_Delete || code == kVK_ForwardDelete)
    return v1::USER_ACTION_FORGET;
  const auto text = printable(raw.lowercaseString);
  if (shift && text == "u")
    return v1::USER_ACTION_START_UNICODE_INPUT;
  if (text == "h")
    return v1::USER_ACTION_BACKSPACE;
  if (text == "p")
    return v1::USER_ACTION_UP;
  if (text == "m")
    return v1::USER_ACTION_ENTER;
  if (text == "n")
    return v1::USER_ACTION_DOWN;
  if (text == "f" || text == "o")
    return v1::USER_ACTION_RIGHT;
  if (text == "i")
    return v1::USER_ACTION_LEFT;
  if (text == "l")
    return v1::USER_ACTION_FULL_WIDTH_ROMAN;
  if (text == "j")
    return v1::USER_ACTION_HIRAGANA;
  if (text == "k")
    return v1::USER_ACTION_KATAKANA;
  if (text == ";")
    return v1::USER_ACTION_HALF_WIDTH_KATAKANA;
  if (text == ":" || text == "'")
    return v1::USER_ACTION_HALF_WIDTH_ROMAN;
  return v1::USER_ACTION_CONSUME;
}
v1::UserAction action(NSEvent *event) {
  if (event.type != NSEventTypeKeyDown ||
      (event.modifierFlags & NSEventModifierFlagCommand))
    return v1::USER_ACTION_UNSPECIFIED;
  if (event.modifierFlags & NSEventModifierFlagControl)
    return controlAction(event.charactersIgnoringModifiers, event.keyCode,
                         event.modifierFlags & NSEventModifierFlagShift);
  switch (event.keyCode) {
  case kVK_Delete:
    return v1::USER_ACTION_BACKSPACE;
  case kVK_ForwardDelete:
    return v1::USER_ACTION_DELETE_FORWARD;
  case kVK_Return:
  case kVK_ANSI_KeypadEnter:
    return v1::USER_ACTION_ENTER;
  case kVK_Escape:
    return v1::USER_ACTION_ESCAPE;
  case kVK_Space:
    return v1::USER_ACTION_SPACE;
  case kVK_Tab:
    return v1::USER_ACTION_TAB;
  case kVK_LeftArrow:
    return v1::USER_ACTION_LEFT;
  case kVK_RightArrow:
    return v1::USER_ACTION_RIGHT;
  case kVK_UpArrow:
    return v1::USER_ACTION_UP;
  case kVK_DownArrow:
    return v1::USER_ACTION_DOWN;
  case kVK_PageUp:
    return v1::USER_ACTION_PAGE_UP;
  case kVK_PageDown:
    return v1::USER_ACTION_PAGE_DOWN;
  case kVK_JIS_Eisu:
    return v1::USER_ACTION_EISU;
  case kVK_JIS_Kana:
    return v1::USER_ACTION_KANA;
  case kVK_F6:
    return v1::USER_ACTION_HIRAGANA;
  case kVK_F7:
    return v1::USER_ACTION_KATAKANA;
  case kVK_F8:
    return v1::USER_ACTION_HALF_WIDTH_KATAKANA;
  case kVK_F9:
    return v1::USER_ACTION_FULL_WIDTH_ROMAN;
  case kVK_F10:
    return v1::USER_ACTION_HALF_WIDTH_ROMAN;
  default:
    return printable(event.characters).empty() ? v1::USER_ACTION_UNSPECIFIED
                                               : v1::USER_ACTION_INPUT;
  }
}
} // namespace

v1::KeyEvent keyEvent(NSEvent *event) {
  v1::KeyEvent key;
  key.set_action(action(event));
  const auto modifiers = event.modifierFlags;
  key.set_shift(modifiers & NSEventModifierFlagShift);
  const auto raw = printable(event.charactersIgnoringModifiers);
  if ((modifiers & NSEventModifierFlagControl) && (raw == "i" || raw == "o"))
    key.set_shift(true);
  key.set_option(
      (modifiers & NSEventModifierFlagOption) &&
      !(modifiers & (NSEventModifierFlagControl | NSEventModifierFlagCommand)));
  const auto text = printable(event.characters);
  if (key.action() == v1::USER_ACTION_INPUT &&
      !(modifiers & (NSEventModifierFlagOption | NSEventModifierFlagControl |
                     NSEventModifierFlagCommand)))
    key.set_text(japanese(text));
  key.set_input(key.option() ? raw : (text.empty() ? raw : text));
  key.set_intention(japanese(raw));
  return key;
}
} // namespace beankey::macos
