#include "key_event.h"
#include "text_input.h"
#import <Carbon/Carbon.h>
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
int main() {
  @autoreleasepool {
    using namespace beankey;
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
    require(macos::keyEvent(event(kVK_ANSI_Minus, @"-", @"-")).text() == "ー",
            "Japanese long vowel mapping");
    require(macos::keyEvent(event(kVK_Return, @"\r", @"\r")).action() ==
                v1::USER_ACTION_ENTER,
            "Return");
    require(macos::keyEvent(event(kVK_ForwardDelete, @"", @"")).action() ==
                v1::USER_ACTION_DELETE_FORWARD,
            "Forward delete");
    require(macos::keyEvent(
                event(kVK_ANSI_U, @"U", @"U",
                      NSEventModifierFlagControl | NSEventModifierFlagShift))
                    .action() == v1::USER_ACTION_START_UNICODE_INPUT,
            "Ctrl Shift U");
    const auto option = macos::keyEvent(
        event(kVK_ANSI_A, @"å", @"a", NSEventModifierFlagOption));
    require(option.option() && option.input() == "a" && option.text().empty(),
            "Option carries the original alphanumeric key");
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
