#pragma once
#include "beankey.pb.h"
#import <InputMethodKit/InputMethodKit.h>
#include <optional>

namespace beankey::macos {
NSString *string(const std::string &utf8);
std::optional<NSUInteger> utf16Offset(NSString *text, NSUInteger scalar);
std::optional<NSUInteger> scalarOffset(NSString *text, NSUInteger utf16);
v1::SurroundingText surroundingText(id<IMKTextInput> client);
bool applyText(const v1::StateResponse &state, id<IMKTextInput> client,
               bool previousMarked);
} // namespace beankey::macos
