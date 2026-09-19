#include "text_input.h"
#include <algorithm>

namespace beankey::macos {
NSString *string(const std::string &utf8) {
  return [[NSString alloc] initWithBytes:utf8.data()
                                  length:utf8.size()
                                encoding:NSUTF8StringEncoding];
}
std::optional<NSUInteger> utf16Offset(NSString *text, NSUInteger scalar) {
  if (!text)
    return std::nullopt;
  NSUInteger offset = 0;
  for (NSUInteger index = 0; index < scalar; ++index) {
    if (offset == text.length)
      return std::nullopt;
    const unichar value = [text characterAtIndex:offset++];
    if (CFStringIsSurrogateHighCharacter(value)) {
      if (offset == text.length ||
          !CFStringIsSurrogateLowCharacter([text characterAtIndex:offset]))
        return std::nullopt;
      ++offset;
    } else if (CFStringIsSurrogateLowCharacter(value))
      return std::nullopt;
  }
  return offset;
}
std::optional<NSUInteger> scalarOffset(NSString *text, NSUInteger utf16) {
  if (!text || utf16 > text.length)
    return std::nullopt;
  NSUInteger scalars = 0;
  for (NSUInteger offset = 0; offset < utf16; ++offset, ++scalars) {
    const unichar value = [text characterAtIndex:offset];
    if (CFStringIsSurrogateHighCharacter(value)) {
      if (++offset >= utf16 ||
          !CFStringIsSurrogateLowCharacter([text characterAtIndex:offset]))
        return std::nullopt;
    } else if (CFStringIsSurrogateLowCharacter(value))
      return std::nullopt;
  }
  return scalars;
}

v1::SurroundingText surroundingText(id<IMKTextInput> client) {
  v1::SurroundingText result;
  NSRange selection = [client selectedRange];
  const NSRange marked = [client markedRange];
  const bool composing = marked.location != NSNotFound &&
                         marked.length != NSNotFound && marked.length > 0;
  if (composing)
    selection = marked;
  if (selection.location == NSNotFound || selection.length == NSNotFound ||
      selection.length > 4096)
    return result;
  // The daemon model context is 512 tokens. Bound IPC even for large documents.
  const NSUInteger start =
      selection.location > 1024 ? selection.location - 1024 : 0;
  NSRange actual = NSMakeRange(NSNotFound, 0);
  NSString *text =
      [client stringFromRange:NSMakeRange(start, selection.location - start +
                                                     selection.length + 1024)
                  actualRange:&actual];
  if (!text || actual.location == NSNotFound ||
      actual.location > selection.location)
    return result;
  const NSUInteger anchor = selection.location - actual.location;
  if (anchor > text.length || selection.length > text.length - anchor)
    return result;
  NSUInteger cursor = anchor + selection.length;
  if (composing) {
    text = [text
        stringByReplacingCharactersInRange:NSMakeRange(anchor, selection.length)
                                withString:@""];
    cursor = anchor;
  }
  const auto scalarCursor = scalarOffset(text, cursor);
  const auto scalarAnchor = scalarOffset(text, anchor);
  if (!scalarCursor || !scalarAnchor || !text.UTF8String)
    return result;
  result.set_available(true);
  result.set_text(text.UTF8String);
  result.set_cursor(*scalarCursor);
  result.set_anchor(*scalarAnchor);
  return result;
}

bool applyText(const v1::StateResponse &state, id<IMKTextInput> client,
               bool previousMarked) {
  NSString *preedit = string(state.preedit());
  NSString *commit = string(state.commit());
  const auto cursor = utf16Offset(preedit, state.preedit_cursor());
  const auto highlighted =
      utf16Offset(preedit, state.highlighted_preedit_length());
  if (!preedit || !commit || !cursor || !highlighted)
    return false;
  const NSRange current = NSMakeRange(NSNotFound, NSNotFound);
  if (commit.length) {
    [client insertText:commit replacementRange:current];
  }
  if (preedit.length) {
    NSMutableAttributedString *marked =
        [[NSMutableAttributedString alloc] initWithString:preedit];
    const NSArray *supported = [client validAttributesForMarkedText];
    if ([supported containsObject:NSUnderlineStyleAttributeName]) {
      [marked addAttribute:NSUnderlineStyleAttributeName
                     value:@(NSUnderlineStyleSingle)
                     range:NSMakeRange(0, preedit.length)];
      if (*highlighted)
        [marked addAttribute:NSUnderlineStyleAttributeName
                       value:@(NSUnderlineStyleThick)
                       range:NSMakeRange(0, *highlighted)];
    }
    [client setMarkedText:marked
           selectionRange:NSMakeRange(*cursor, 0)
         replacementRange:current];
  } else if (previousMarked && commit.length == 0) {
    [client setMarkedText:@""
           selectionRange:NSMakeRange(0, 0)
         replacementRange:current];
  }
  return true;
}
} // namespace beankey::macos
