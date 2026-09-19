#include "candidate_panel.h"
#include "text_input.h"
#include <algorithm>
#include <vector>

@interface BKCandidateLabel : NSTextField
@end
@implementation BKCandidateLabel
- (NSView *)hitTest:(NSPoint)point {
  (void)point;
  return nil;
}
@end

@interface BKCandidateRow : NSButton
@property(nonatomic, copy) void (^forget)(void);
@property(nonatomic, copy) void (^correct)(void);
@end
@implementation BKCandidateRow
- (BOOL)acceptsFirstResponder {
  return NO;
}
- (BOOL)acceptsFirstMouse:(NSEvent *)event {
  (void)event;
  return YES;
}
- (NSMenu *)menuForEvent:(NSEvent *)event {
  (void)event;
  NSMenu *menu = [[NSMenu alloc] initWithTitle:@""];
  if (self.forget) {
    NSMenuItem *item = [menu addItemWithTitle:@"この候補を忘れる"
                                       action:@selector(forgetAction:)
                                keyEquivalent:@""];
    item.target = self;
  }
  if (self.correct) {
    NSMenuItem *item = [menu addItemWithTitle:@"入力を訂正する"
                                       action:@selector(correctAction:)
                                keyEquivalent:@""];
    item.target = self;
  }
  return menu.numberOfItems ? menu : nil;
}
- (void)forgetAction:(id)sender {
  (void)sender;
  if (self.forget)
    self.forget();
}
- (void)correctAction:(id)sender {
  (void)sender;
  if (self.correct)
    self.correct();
}
@end

@implementation BKCandidatePanel {
  std::vector<uint32_t> _indices;
  int _windowStart;
  BOOL _selecting;
}
- (instancetype)init {
  self = [super initWithContentRect:NSZeroRect
                          styleMask:NSWindowStyleMaskBorderless |
                                    NSWindowStyleMaskNonactivatingPanel
                            backing:NSBackingStoreBuffered
                              defer:NO];
  if (self) {
    self.floatingPanel = YES;
    self.becomesKeyOnlyIfNeeded = YES;
    self.hidesOnDeactivate = NO;
    self.releasedWhenClosed = NO;
    self.backgroundColor = NSColor.whiteColor;
    self.hasShadow = YES;
    self.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                              NSWindowCollectionBehaviorFullScreenAuxiliary;
    self.accessibilityLabel = @"beanKey 変換候補";
  }
  return self;
}
- (BOOL)canBecomeKeyWindow {
  return NO;
}
- (BOOL)canBecomeMainWindow {
  return NO;
}
- (std::optional<uint32_t>)candidateForDigit:(NSUInteger)digit {
  if (!_selecting || digit >= _indices.size())
    return std::nullopt;
  return _indices[digit];
}
- (void)reset {
  _indices.clear();
  _windowStart = 0;
  _selecting = NO;
  [self orderOut:nil];
}
- (void)choose:(NSButton *)sender {
  const auto index = static_cast<size_t>(sender.tag);
  if (self.selectCandidate && index < _indices.size())
    self.selectCandidate(_indices[index]);
}
- (void)displayRows:(NSArray<NSString *> *)rows
        annotations:(NSArray<NSString *> *)annotations
           selected:(NSInteger)selected
         prediction:(NSString *)prediction
             client:(id<IMKTextInput>)client
           learning:(BOOL)learning
               typo:(BOOL)typo {
  if (!rows.count && !prediction.length) {
    [self orderOut:nil];
    return;
  }
  const CGFloat rowHeight = 30;
  const NSFont *font = [NSFont systemFontOfSize:13];
  CGFloat width = 160;
  for (NSUInteger i = 0; i < rows.count; ++i) {
    const CGFloat textWidth =
        [rows[i] sizeWithAttributes:@{NSFontAttributeName : font}].width;
    const CGFloat annotationWidth =
        [annotations[i] sizeWithAttributes:@{NSFontAttributeName : font}].width;
    width = std::max(width, 48 + textWidth +
                                (annotationWidth ? 16 + annotationWidth : 0));
  }
  if (prediction.length)
    width = std::max(
        width,
        20 + [prediction sizeWithAttributes:@{NSFontAttributeName : font}]
                 .width);
  NSRect caret = NSZeroRect;
  [client attributesForCharacterIndex:0 lineHeightRectangle:&caret];
  NSScreen *screen = nil;
  for (NSScreen *candidate in NSScreen.screens)
    if (NSIntersectsRect(candidate.frame, NSInsetRect(caret, -1, -1))) {
      screen = candidate;
      break;
    }
  if (!screen)
    screen = NSScreen.mainScreen;
  if (!screen)
    return;
  const NSRect visible = screen.visibleFrame;
  width = std::min(width, visible.size.width);
  const CGFloat height = rowHeight * (rows.count + (prediction.length ? 1 : 0));
  NSView *content =
      [[NSView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
  NSColor *normal = [NSColor colorWithSRGBRed:74.0 / 255
                                        green:74.0 / 255
                                         blue:74.0 / 255
                                        alpha:1];
  NSColor *label = [NSColor colorWithSRGBRed:107.0 / 255
                                       green:107.0 / 255
                                        blue:112.0 / 255
                                       alpha:1];
  for (NSUInteger index = 0; index < rows.count; ++index) {
    const BOOL highlighted = static_cast<NSInteger>(index) == selected;
    BKCandidateRow *row = [[BKCandidateRow alloc]
        initWithFrame:NSMakeRect(0, height - rowHeight * (index + 1), width,
                                 rowHeight)];
    row.bordered = NO;
    row.title = @"";
    row.tag = index;
    row.target = self;
    row.action = @selector(choose:);
    row.wantsLayer = YES;
    row.layer.backgroundColor =
        (highlighted ? [NSColor colorWithSRGBRed:0
                                           green:110.0 / 255
                                            blue:220.0 / 255
                                           alpha:1]
                     : NSColor.whiteColor)
            .CGColor;
    const uint32_t sourceIndex = _indices[index];
    __weak BKCandidatePanel *weakSelf = self;
    if (learning)
      row.forget = ^{
        if (weakSelf.forgetCandidate)
          weakSelf.forgetCandidate(sourceIndex);
      };
    if (typo)
      row.correct = ^{
        if (weakSelf.requestTypoCorrections)
          weakSelf.requestTypoCorrections();
      };
    const CGFloat annotationWidth =
        annotations[index].length
            ? std::min(width / 2,
                       [annotations[index]
                           sizeWithAttributes:@{NSFontAttributeName : font}]
                           .width)
            : 0;
    const NSArray *texts = @[
      _selecting ? [NSString stringWithFormat:@"%lu", (unsigned long)index + 1]
                 : @"",
      rows[index], annotations[index]
    ];
    const CGFloat starts[] = {10, 30, width - annotationWidth - 10};
    const CGFloat widths[] = {
        16, std::max<CGFloat>(0, width - annotationWidth - 50),
        annotationWidth};
    for (NSUInteger part = 0; part < 3; ++part) {
      NSTextField *field = [BKCandidateLabel labelWithString:texts[part]];
      field.frame = NSMakeRect(starts[part], 6, widths[part], 18);
      field.font = (NSFont *)font;
      field.textColor =
          highlighted ? NSColor.whiteColor : (part == 0 ? label : normal);
      field.lineBreakMode = NSLineBreakByTruncatingTail;
      [row addSubview:field];
    }
    row.accessibilityLabel =
        [NSString stringWithFormat:@"%@ %@", rows[index], annotations[index]];
    [content addSubview:row];
  }
  if (prediction.length) {
    NSTextField *field = [NSTextField labelWithString:prediction];
    field.frame = NSMakeRect(10, 6, width - 20, 18);
    field.font = (NSFont *)font;
    field.textColor = normal;
    field.lineBreakMode = NSLineBreakByTruncatingTail;
    [content addSubview:field];
  }
  self.contentView = content;
  CGFloat y = NSMinY(caret) - height;
  if (y < NSMinY(visible))
    y = NSMaxY(caret);
  y = std::clamp(y, NSMinY(visible),
                 std::max(NSMinY(visible), NSMaxY(visible) - height));
  const CGFloat x =
      std::clamp(NSMinX(caret), NSMinX(visible), NSMaxX(visible) - width);
  [self setFrame:NSMakeRect(x, y, width, height) display:YES];
  self.level =
      std::max<NSInteger>(NSPopUpMenuWindowLevel, [client windowLevel] + 1);
  [self orderFrontRegardless];
}
- (void)showState:(const beankey::v1::StateResponse &)state
           client:(id<IMKTextInput>)client {
  using namespace beankey;
  _selecting = state.candidate_window() == v1::CANDIDATE_WINDOW_SELECTING;
  if (!_selecting)
    _windowStart = 0;
  else if (state.selected_candidate() >= 0) {
    if (state.selected_candidate() < _windowStart)
      _windowStart = state.selected_candidate();
    else if (state.selected_candidate() >= _windowStart + 9)
      _windowStart = state.selected_candidate() - 8;
  }
  _indices.clear();
  NSMutableArray *rows = [NSMutableArray array];
  NSMutableArray *annotations = [NSMutableArray array];
  if (state.candidate_window() != v1::CANDIDATE_WINDOW_HIDDEN) {
    const int end =
        std::min(state.candidates_size(), _windowStart + (_selecting ? 9 : 1));
    for (int i = _windowStart; i < end; ++i) {
      const auto &candidate = state.candidates(i);
      [rows addObject:macos::string(candidate.text()) ?: @""];
      [annotations addObject:macos::string(candidate.annotation()) ?: @""];
      _indices.push_back(candidate.index());
    }
  }
  NSString *prediction =
      state.has_prediction()
          ? [@"→ "
                stringByAppendingString:macos::string(
                                            state.prediction().display_text())
                                            ?: @""]
          : @"";
  [self displayRows:rows
        annotations:annotations
           selected:state.selected_candidate() - _windowStart
         prediction:prediction
             client:client
           learning:state.learning_writable()
               typo:state.lm_typo_available()];
}
- (void)showTypos:(const beankey::v1::TypoCorrectionResponse &)typos
           client:(id<IMKTextInput>)client {
  _indices.clear();
  _windowStart = 0;
  _selecting = YES;
  NSMutableArray *rows = [NSMutableArray array];
  NSMutableArray *annotations = [NSMutableArray array];
  for (int i = 0; i < std::min(9, typos.candidates_size()); ++i) {
    [rows
        addObject:beankey::macos::string(typos.candidates(i).corrected_input())
                      ?: @""];
    [annotations
        addObject:beankey::macos::string(typos.candidates(i).converted_text())
                      ?: @""];
    _indices.push_back(i);
  }
  [self displayRows:rows
        annotations:annotations
           selected:0
         prediction:@""
             client:client
           learning:NO
               typo:NO];
}
@end
