#include "key_event.h"
#include "text_input.h"
#import <Carbon/Carbon.h>
#include <array>
#include <cstdlib>
#include <iostream>

void require(bool value, const char *message) {
  if (!value) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}
NSEvent *event(unsigned short code, NSString *characters, NSString *raw,
               NSEventModifierFlags modifiers = 0) {
  return [NSEvent keyEventWithType:NSEventTypeKeyDown
                          location:NSZeroPoint
                     modifierFlags:modifiers
                         timestamp:0
                      windowNumber:0
                           context:nil
                        characters:characters
       charactersIgnoringModifiers:raw
                         isARepeat:NO
                           keyCode:code];
}
void controlShortcuts() {
  using namespace beankey::v1;
  struct Shortcut {
    unsigned short code;
    NSString *logical;
    UserAction action;
    bool editSegment = false;
  };
  const Shortcut shortcuts[] = {
      {kVK_ANSI_H, @"h", USER_ACTION_BACKSPACE},
      {kVK_ANSI_P, @"p", USER_ACTION_UP},
      {kVK_ANSI_M, @"m", USER_ACTION_ENTER},
      {kVK_ANSI_N, @"n", USER_ACTION_DOWN},
      {kVK_ANSI_F, @"f", USER_ACTION_RIGHT},
      {kVK_ANSI_I, @"i", USER_ACTION_LEFT, true},
      {kVK_ANSI_O, @"o", USER_ACTION_RIGHT, true},
      {kVK_ANSI_L, @"l", USER_ACTION_FULL_WIDTH_ROMAN},
      {kVK_ANSI_J, @"j", USER_ACTION_HIRAGANA},
      {kVK_ANSI_K, @"k", USER_ACTION_KATAKANA},
      {kVK_ANSI_Semicolon, @";", USER_ACTION_HALF_WIDTH_KATAKANA},
      {kVK_ANSI_Semicolon, @":", USER_ACTION_HALF_WIDTH_ROMAN},
      {kVK_ANSI_Quote, @"'", USER_ACTION_HALF_WIDTH_ROMAN},
  };
  const auto check = [](unsigned short code, NSString *logical,
                        NSEventModifierFlags modifiers, UserAction action,
                        bool shift) {
    const auto key = beankey::macos::keyEvent(
        event(code, logical, logical, modifiers));
    require(key.action() == action && key.shift() == shift &&
                key.text().empty() && !key.option(),
            "Control shortcut matches pinned Desktop action and modifiers");
  };
  for (const auto &shortcut : shortcuts) {
    for (NSString *logical in @[shortcut.logical,
                               shortcut.logical.uppercaseString]) {
      for (const auto modifiers : std::array<NSEventModifierFlags, 2>{
               NSEventModifierFlagControl,
               NSEventModifierFlagControl | NSEventModifierFlagCapsLock}) {
        check(shortcut.code, logical, modifiers, shortcut.action,
              shortcut.editSegment);
      }
      check(shortcut.code, logical,
            NSEventModifierFlagControl | NSEventModifierFlagShift,
            USER_ACTION_CONSUME, true);
      check(shortcut.code, logical,
            NSEventModifierFlagControl | NSEventModifierFlagOption,
            USER_ACTION_UNSPECIFIED, false);
    }
  }
  for (NSString *logical in @[@"u", @"U"]) {
    check(kVK_ANSI_U, logical,
          NSEventModifierFlagControl | NSEventModifierFlagShift,
          USER_ACTION_START_UNICODE_INPUT, true);
    check(kVK_ANSI_U, logical,
          NSEventModifierFlagControl | NSEventModifierFlagCapsLock,
          USER_ACTION_CONSUME, false);
    check(kVK_ANSI_U, logical,
          NSEventModifierFlagControl | NSEventModifierFlagOption |
              NSEventModifierFlagShift,
          USER_ACTION_UNSPECIFIED, true);
  }
  for (const auto code : std::array<unsigned short, 6>{
           kVK_Return, kVK_Space, kVK_LeftArrow, kVK_F6, kVK_ANSI_1, kVK_ANSI_Q}) {
    check(code, @"", NSEventModifierFlagControl, USER_ACTION_CONSUME, false);
  }
  check(kVK_Delete, @"", NSEventModifierFlagControl, USER_ACTION_FORGET, false);
  check(kVK_Delete, @"",
        NSEventModifierFlagControl | NSEventModifierFlagShift,
        USER_ACTION_FORGET, true);
  check(kVK_Delete, @"",
        NSEventModifierFlagControl | NSEventModifierFlagOption,
        USER_ACTION_UNSPECIFIED, false);
}

int main() {
  @autoreleasepool {
    using namespace beankey;
    controlShortcuts();
    NSString *text = @"a日本😀e\u0301👩‍💻";
    require(macos::utf16Offset(text, 4) == 5,
            "Unicode scalar offsets must include surrogate pairs");
    require(macos::scalarOffset(text, 4) == std::nullopt,
            "Reject a UTF-16 offset inside a surrogate pair");
    for (NSUInteger scalar = 0; scalar <= 9; ++scalar) {
      const auto offset = macos::utf16Offset(text, scalar);
      require(offset && macos::scalarOffset(text, *offset) == scalar,
              "Unicode scalar/UTF-16 roundtrip");
    }
    require(!macos::utf16Offset(text, 10),
            "Reject out of range scalar offsets");
    const auto hyphen = macos::keyEvent(event(kVK_ANSI_Minus, @"-", @"-"));
    require(hyphen.action() == v1::USER_ACTION_INPUT && hyphen.text() == "-" &&
                hyphen.input() == "-" && hyphen.intention() == "-",
            "Hyphen reaches the daemon without Japanese normalization");
    require(macos::keyEvent(event(kVK_Return, @"\r", @"\r")).action() ==
                v1::USER_ACTION_ENTER,
            "Return");
    require(macos::keyEvent(event(kVK_ForwardDelete, @"", @"")).action() ==
                v1::USER_ACTION_UNSPECIFIED,
            "Forward delete passes through like Fcitx5");
    require(macos::keyEvent(
                event(kVK_ANSI_U, @"U", @"U",
                      NSEventModifierFlagControl | NSEventModifierFlagShift))
                    .action() == v1::USER_ACTION_START_UNICODE_INPUT,
            "Ctrl Shift U");
    const auto option = macos::keyEvent(
        event(kVK_ANSI_A, @"å", @"a", NSEventModifierFlagOption));
    require(option.action() == v1::USER_ACTION_INPUT && option.option() &&
                option.input() == "å" && option.intention() == "a" &&
                option.text().empty(),
            "Option preserves generated text separately from its logical key");
    const auto shiftedOption = macos::keyEvent(
        event(kVK_ANSI_A, @"Å", @"A",
              NSEventModifierFlagOption | NSEventModifierFlagShift));
    require(shiftedOption.option() && shiftedOption.shift() &&
                shiftedOption.input() == "Å" &&
                shiftedOption.intention() == "A" &&
                shiftedOption.text().empty(),
            "Shift Option preserves generated text and shifted logical text");
    const auto optionMinus = macos::keyEvent(
        event(kVK_ANSI_Minus, @"¯", @"-", NSEventModifierFlagOption));
    require(optionMinus.input() == "¯" && optionMinus.intention() == "-",
            "Option punctuation remains generated input until daemon handling");
    const auto shiftedOptionPeriod = macos::keyEvent(
        event(kVK_ANSI_Period, @"˘", @">",
              NSEventModifierFlagOption | NSEventModifierFlagShift));
    require(shiftedOptionPeriod.input() == "˘" &&
                shiftedOptionPeriod.intention() == ">",
            "Option-stripped punctuation retains Shift for direct fullwidth");
    require(macos::keyEvent(
                event(kVK_ANSI_C, @"c", @"c", NSEventModifierFlagCommand))
                    .action() == v1::USER_ACTION_UNSPECIFIED,
            "Command shortcuts pass through");
    require(macos::keyEvent(event(kVK_JIS_Kana, @"", @"")).action() ==
                v1::USER_ACTION_KANA,
            "Kana key");
    require(macos::keyEvent(event(kVK_JIS_Eisu, @"", @"")).action() ==
                v1::USER_ACTION_EISU,
            "Eisu key");
    require(macos::keyEvent(
                event(kVK_ANSI_I, @"i", @"i", NSEventModifierFlagControl))
                .shift(),
            "Ctrl I changes clause boundary like Fcitx5");
  }
}
