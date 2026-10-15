#include "candidate_panel.h"
#include "input_session.h"
#include "text_input.h"
#import <Carbon/Carbon.h>
#include <algorithm>
#include <array>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <map>
#include <sys/socket.h>
#include <sys/un.h>
#include <thread>
#include <unistd.h>
#include <vector>

static void require(bool value, const char *message) {
  if (!value) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}

// Exercise Cocoa's actual marked-text implementation behind the IMK client API.
@interface BKTestTextView : NSTextView
@end
@implementation BKTestTextView
- (void)setMarkedText:(id)text
       selectionRange:(NSRange)selection
     replacementRange:(NSRange)replacement {
  [self setMarkedText:text
         selectedRange:selection
      replacementRange:replacement];
}
- (NSArray *)validAttributesForMarkedText {
  return @[ NSUnderlineStyleAttributeName ];
}
- (NSString *)stringFromRange:(NSRange)range
                  actualRange:(NSRangePointer)actual {
  if (range.location > self.string.length)
    return nil;
  *actual = NSIntersectionRange(range, NSMakeRange(0, self.string.length));
  return [self.string substringWithRange:*actual];
}
- (NSDictionary *)attributesForCharacterIndex:(NSUInteger)index
                          lineHeightRectangle:(NSRect *)rect {
  (void)index;
  *rect = NSMakeRect(100, 300, 1, 20);
  return @{};
}
- (NSInteger)windowLevel {
  return NSNormalWindowLevel;
}
- (NSString *)bundleIdentifier {
  return @"beanKey.tests";
}
@end

static NSEvent *key(unsigned short code, NSString *text,
                    NSEventModifierFlags modifiers = 0) {
  return [NSEvent keyEventWithType:NSEventTypeKeyDown
                          location:NSZeroPoint
                     modifierFlags:modifiers
                         timestamp:0
                      windowNumber:0
                           context:nil
                        characters:text
       charactersIgnoringModifiers:text
                         isARepeat:NO
                           keyCode:code];
}
static void waitReady(BKDaemonConnection *connection) {
  [connection prepare];
  NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:10];
  while (!connection.ready && deadline.timeIntervalSinceNow > 0)
    [[NSRunLoop currentRunLoop]
        runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
  require(connection.ready, "The socket must become ready asynchronously");
}
static bool transfer(int socket, void *buffer, size_t size, bool writing) {
  auto *bytes = static_cast<char *>(buffer);
  while (size) {
    const ssize_t count =
        writing ? write(socket, bytes, size) : read(socket, bytes, size);
    if (count <= 0)
      return false;
    bytes += count;
    size -= count;
  }
  return true;
}
static bool readFrame(int socket, beankey::v1::Envelope &envelope) {
  uint32_t size = 0;
  for (unsigned shift = 0; shift <= 28; shift += 7) {
    uint8_t byte;
    if (!transfer(socket, &byte, 1, false))
      return false;
    size |= (byte & 0x7fU) << shift;
    if (!(byte & 0x80U)) {
      require(size < 1024 * 1024, "bounded test request");
      std::string payload(size, '\0');
      return transfer(socket, payload.data(), size, false) &&
             envelope.ParseFromString(payload);
    }
  }
  return false;
}
static void writeFrame(int socket, const beankey::v1::Envelope &envelope) {
  std::string payload = envelope.SerializeAsString();
  uint32_t size = payload.size();
  do {
    uint8_t byte = size & 0x7f;
    size >>= 7;
    if (size)
      byte |= 0x80;
    require(transfer(socket, &byte, 1, true), "write prefix");
  } while (size);
  require(transfer(socket, payload.data(), payload.size(), true),
          "write frame");
}

static void fakeDaemonTest() {
  using namespace beankey;
  std::array<char, 64> pattern{};
  std::strcpy(pattern.data(), "/private/tmp/beankey-macos-test.XXXXXX");
  require(mkdtemp(pattern.data()) != nullptr, "temporary socket directory");
  std::string path = std::string(pattern.data()) + "/test.sock";
  const int listener = socket(AF_UNIX, SOCK_STREAM, 0);
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  std::strcpy(address.sun_path, path.c_str());
  require(bind(listener, reinterpret_cast<sockaddr *>(&address),
               sizeof(address)) == 0 &&
              listen(listener, 1) == 0,
          "listen");
  std::vector<v1::Envelope> requests;
  std::thread server([listener, &requests] {
    const int socket = accept(listener, nullptr, nullptr);
    std::map<std::string, std::string> sessions;
    int typoRequests = 0;
    v1::Envelope request;
    while (readFrame(socket, request)) {
      requests.push_back(request);
      v1::Envelope response;
      response.set_protocol_version(1);
      response.set_request_id(request.request_id());
      response.set_session_id(request.session_id());
      if (request.has_request_typo_corrections()) {
        ++typoRequests;
        // Exercise twelve and ten candidates, plus an empty refresh of an
        // already-visible list (the seventh request).
        const int count = typoRequests == 7 ? 0 : typoRequests == 2 ? 10 : 12;
        auto *typos = response.mutable_typo_correction_response();
        for (int i = 0; i < count; ++i) {
          auto *candidate = typos->add_candidates();
          candidate->set_corrected_input("訂正" + std::to_string(i));
          candidate->set_converted_text("候補" + std::to_string(i));
        }
        writeFrame(socket, response);
        continue;
      }
      auto *state = response.mutable_state_response();
      state->set_consumed(true);
      state->set_selected_candidate(-1);
      state->set_candidate_window(v1::CANDIDATE_WINDOW_HIDDEN);
      auto &preedit = sessions[request.session_id()];
      if (request.has_key_event() && request.key_event().text() == "x") {
        response.set_request_id(request.request_id() +
                                1); // must reset, not apply
      } else if (request.has_select_candidate() ||
                 request.has_commit_composition() ||
                 (request.has_key_event() &&
                  request.key_event().action() == v1::USER_ACTION_ENTER)) {
        state->set_commit(preedit);
        preedit.clear();
      } else if (request.has_select_typo_correction()) {
        preedit = "訂正" + std::to_string(request.select_typo_correction().index());
      } else if (request.has_key_event()) {
        if (request.key_event().action() == v1::USER_ACTION_ESCAPE)
          preedit.clear();
        else
          preedit = request.key_event().text() == "b" ? "「」" : "仮名😀";
      }
      if (!preedit.empty()) {
        state->set_candidate_window(v1::CANDIDATE_WINDOW_SELECTING);
        state->set_selected_candidate(0);
        auto *candidate = state->add_candidates();
        candidate->set_text(preedit);
        candidate->set_index(0);
        if (preedit == "「」") {
          // Legacy field 4 must not change the normal text insertion position.
          require(candidate->MergeFromString("\x22\x02\x08\x01"),
                  "legacy cursor action fixture");
        }
      }
      state->set_preedit(preedit);
      state->set_preedit_cursor(preedit.empty()     ? 0
                                : preedit == "「」" ? 2
                                                    : 3);
      state->set_highlighted_preedit_length(preedit.empty() ? 0 : 2);
      writeFrame(socket, response);
    }
    close(socket);
    close(listener);
  });
  BKDaemonConnection *connection =
      [[BKDaemonConnection alloc] initWithSocketPath:macos::string(path)
                                            launcher:^{
                                            }];
  waitReady(connection);
  BKInputSession *session =
      [[BKInputSession alloc] initWithConnection:connection];
  BKTestTextView *view =
      [[BKTestTextView alloc] initWithFrame:NSMakeRect(0, 0, 500, 300)];
  id<IMKTextInput> client = (id<IMKTextInput>)view;
  view.string = @"前😀選択後";
  view.selectedRange = NSMakeRange(3, 2);
  const auto surrounding = macos::surroundingText(client);
  require(surrounding.available() && surrounding.text() == "前😀選択後" &&
              surrounding.anchor() == 2 && surrounding.cursor() == 4,
          "selected surrounding text uses scalar offsets");
  view.string = @"";
  view.selectedRange = NSMakeRange(0, 0);
  [session activate];
  require([session handleEvent:key(kVK_ANSI_A, @"a") client:client],
          "consume input");
  require([view.string isEqualToString:@"仮名😀"] && view.hasMarkedText &&
              view.selectedRange.location == 4,
          "Cocoa marked text and emoji cursor");
  require([session handleEvent:key(kVK_ANSI_1, @"1") client:client],
          "number candidate selection");
  require([view.string isEqualToString:@"仮名😀"] && !view.hasMarkedText,
          "candidate commit without duplicate text");
  require([session handleEvent:key(kVK_ANSI_A, @"a") client:client],
          "composition for mouse selection");
  BKCandidatePanel *panel = nil;
  for (NSWindow *window in NSApp.windows)
    if ([window isKindOfClass:BKCandidatePanel.class] && window.visible)
      panel = (BKCandidatePanel *)window;
  require(panel && !panel.canBecomeKeyWindow && !panel.canBecomeMainWindow,
          "candidate panel must not steal keyboard focus");
  NSButton *row = (NSButton *)panel.contentView.subviews.firstObject;
  require([row isKindOfClass:NSButton.class],
          "candidate exposes a selection button");
  NSView *label = row.subviews[1];
  const NSPoint labelPoint =
      [row convertPoint:NSMakePoint(NSMidX(label.frame), NSMidY(label.frame))
                 toView:panel.contentView];
  require([panel.contentView hitTest:labelPoint] == row,
          "candidate text must forward clicks to its row");
  [row performClick:nil];
  require([view.string isEqualToString:@"仮名😀仮名😀"] && !view.hasMarkedText,
          "mouse selection commits without duplicate text");

  const auto compose = [&] {
    require([session handleEvent:key(kVK_ANSI_A, @"a") client:client],
            "compose before requesting typo corrections");
    require(![panel selectedTypoCandidate], "normal state clears typo mode");
  };
  const auto requestTypos = [&] {
    panel.requestTypoCorrections();
    require([panel selectedTypoCandidate] == 0,
            "new typo response starts at its first candidate");
  };
  const auto down = [&](int count) {
    for (int i = 0; i < count; ++i)
      require([session handleEvent:key(kVK_DownArrow, @"") client:client],
              "local typo navigation is consumed");
  };

  compose();
  requestTypos(); // twelve candidates
  require(![session handleEvent:key(kVK_PageDown, @"") client:client] &&
              [panel selectedTypoCandidate] == 0,
          "PageDown remains passthrough, not a new paging binding");
  down(9);
  require([panel selectedTypoCandidate] == 9 &&
              [panel candidateForDigit:8] == 9,
          "tenth correction is visible beyond the initial window");
  require([session handleEvent:key(kVK_Space, @" ") client:client],
          "Space selects the next correction locally");
  down(2);
  require([panel selectedTypoCandidate] == 11,
          "forward typo navigation stops at the last correction");
  require([session handleEvent:key(kVK_UpArrow, @"") client:client] &&
              [panel selectedTypoCandidate] == 10,
          "Up selects the previous correction locally");
  require([session handleEvent:key(kVK_Space, @" ", NSEventModifierFlagShift)
                        client:client] &&
              [panel selectedTypoCandidate] == 9,
          "ShiftSpace selects the previous correction locally");
  require([session handleEvent:key(kVK_ANSI_N, @"n", NSEventModifierFlagControl)
                        client:client] &&
              [panel selectedTypoCandidate] == 10,
          "CtrlN reuses normalized Down for typo navigation");
  require([session handleEvent:key(kVK_ANSI_P, @"p", NSEventModifierFlagControl)
                        client:client] &&
              [panel selectedTypoCandidate] == 9,
          "CtrlP reuses normalized Up for typo navigation");
  down(2);
  require([session handleEvent:key(kVK_ANSI_9, @"9") client:client] &&
              [view.string hasSuffix:@"訂正11"] &&
              ![panel selectedTypoCandidate],
          "visible digit nine sends absolute correction index eleven");

  compose();
  requestTypos(); // ten candidates
  down(9);
  row = (NSButton *)panel.contentView.subviews.lastObject;
  [row performClick:nil];
  require([view.string hasSuffix:@"訂正9"] && ![panel selectedTypoCandidate],
          "clicking a shifted window sends absolute correction index nine");

  compose();
  requestTypos();
  down(10);
  require([session handleEvent:key(kVK_Return, @"\r") client:client] &&
              [view.string hasSuffix:@"訂正10"] &&
              ![panel selectedTypoCandidate],
          "Enter selects the locally highlighted correction, not normal input");

  compose();
  requestTypos();
  down(9);
  require([session handleEvent:key(kVK_Escape, @"\033") client:client] &&
              ![panel selectedTypoCandidate] && !panel.visible,
          "Escape keeps the daemon cancellation behavior and clears typo UI");

  compose();
  requestTypos();
  down(9);
  require([session handleEvent:key(kVK_ANSI_B, @"b") client:client] &&
              ![panel selectedTypoCandidate] &&
              [panel candidateForDigit:0] == 0,
          "ordinary input restores a normal, unshifted candidate window");
  require([session handleEvent:key(kVK_ANSI_1, @"1") client:client],
          "normal digit selection after ordinary input");

  compose();
  requestTypos();
  down(9);
  panel.requestTypoCorrections(); // empty refresh
  require(![panel selectedTypoCandidate] &&
              [panel candidateForDigit:0] == 0,
          "empty response discards stale corrections and restores normal rows");
  require([session handleEvent:key(kVK_ANSI_1, @"1") client:client],
          "normal digit selection after an empty typo response");

  compose();
  requestTypos();
  down(9);
  [session activate];
  require(![panel selectedTypoCandidate] && ![panel candidateForDigit:0] &&
              !panel.visible,
          "activation reset discards the complete local typo list");
  down(1); // no longer typo mode: this Down must reach the daemon exactly once
  require(![panel selectedTypoCandidate], "normal Down does not revive typo mode");
  require([session handleEvent:key(kVK_Escape, @"\033") client:client],
          "clear composition before the bracket regression scenarios");

  for (int selection = 0; selection < 2; ++selection) {
    view.string = @"前😀後";
    view.selectedRange = NSMakeRange(3, 0);
    require([session handleEvent:key(kVK_ANSI_B, @"b") client:client],
            "bracket composition");
    require([session handleEvent:selection == 0 ? key(kVK_Return, @"\r")
                                                : key(kVK_ANSI_1, @"1")
                          client:client],
            "bracket selection");
    require([view.string isEqualToString:@"前😀「」後"] &&
                !view.hasMarkedText &&
                NSEqualRanges(view.selectedRange, NSMakeRange(5, 0)),
            "bracket commit leaves the caret after the inserted text");
  }
  view.string = @"仮名😀仮名😀";
  view.selectedRange = NSMakeRange(view.string.length, 0);
  require([session handleEvent:key(kVK_ANSI_A, @"a") client:client],
          "new composition");
  requestTypos();
  down(9);
  require(![session handleEvent:key(kVK_ANSI_X, @"x") client:client],
          "mismatched request must return the key");
  require([view.string isEqualToString:@"仮名😀仮名😀"] && !view.hasMarkedText,
          "failure only clears the active marked text");
  require(!connection.ready, "mismatched response disconnects transport");
  require(![panel selectedTypoCandidate] && ![panel candidateForDigit:0] &&
              !panel.visible,
          "transport failure discards local typo selection and rows");
  [session deactivate:client];
  server.join();
  std::vector<uint32_t> selectedTypos;
  int forwardedDown = 0;
  int forwardedUpOrSpace = 0;
  for (const auto &request : requests) {
    if (request.has_select_typo_correction())
      selectedTypos.push_back(request.select_typo_correction().index());
    if (request.has_key_event()) {
      forwardedDown += request.key_event().action() == v1::USER_ACTION_DOWN;
      forwardedUpOrSpace += request.key_event().action() == v1::USER_ACTION_UP ||
                            request.key_event().action() == v1::USER_ACTION_SPACE;
    }
  }
  require(selectedTypos == std::vector<uint32_t>({11, 9, 10}),
          "digit, click and Enter use exact absolute typo indices on the wire");
  require(forwardedDown == 1 && forwardedUpOrSpace == 0,
          "typo navigation is never forwarded or replayed as ordinary IPC keys");
  unlink(path.c_str());
  rmdir(pattern.data());
}

static void candidateAnnotationTest() {
  BKTestTextView *view =
      [[BKTestTextView alloc] initWithFrame:NSMakeRect(0, 0, 500, 300)];
  beankey::v1::StateResponse state;
  state.set_candidate_window(beankey::v1::CANDIDATE_WINDOW_SELECTING);
  state.set_selected_candidate(1);
  auto *first = state.add_candidates();
  first->set_text("これ");
  first->set_annotation("辞書");
  first->set_index(0);
  auto *second = state.add_candidates();
  second->set_text("これやろうかな");
  second->set_annotation("Zenzai");
  second->set_index(1);
  BKCandidatePanel *panel = [[BKCandidatePanel alloc] init];
  [panel showState:state client:(id<IMKTextInput>)view];
  NSButton *unselectedRow = (NSButton *)panel.contentView.subviews[0];
  NSTextField *unselectedCandidate = (NSTextField *)unselectedRow.subviews[1];
  NSTextField *unselectedAnnotation = (NSTextField *)unselectedRow.subviews[2];
  NSButton *row = (NSButton *)panel.contentView.subviews[1];
  NSTextField *candidate = (NSTextField *)row.subviews[1];
  NSTextField *annotation = (NSTextField *)row.subviews[2];
  require(candidate.frame.size.width >= candidate.fittingSize.width &&
              [annotation.stringValue isEqualToString:@"Zenzai"] &&
              annotation.frame.size.width >= annotation.fittingSize.width,
          "Zenzai annotation must fit without truncation");
  require(
      ![unselectedAnnotation.textColor isEqual:unselectedCandidate.textColor] &&
          annotation.textColor.alphaComponent <
              candidate.textColor.alphaComponent,
      "annotations must be visually quieter than candidate text");
  [panel reset];
}

static beankey::v1::TypoCorrectionResponse typoCandidates(int count) {
  beankey::v1::TypoCorrectionResponse response;
  for (int i = 0; i < count; ++i) {
    auto *candidate = response.add_candidates();
    candidate->set_corrected_input("訂正" + std::to_string(i));
    candidate->set_converted_text("候補" + std::to_string(i));
  }
  return response;
}

static void candidateTypoWindowTest() {
  BKTestTextView *view =
      [[BKTestTextView alloc] initWithFrame:NSMakeRect(0, 0, 500, 300)];
  id<IMKTextInput> client = (id<IMKTextInput>)view;
  BKCandidatePanel *panel = [[BKCandidatePanel alloc] init];
  for (const int count : {0, 1, 5, 9, 10, 12}) {
    auto response = typoCandidates(count);
    [panel showTypos:response client:client];
    response.Clear(); // the panel must retain the entire response, not a view
    for (NSUInteger digit = 0; digit < 9; ++digit) {
      const auto index = [panel candidateForDigit:digit];
      if (digit < static_cast<NSUInteger>(count))
        require(index == digit, "initial typo digits match visible rows");
      else
        require(!index, "no digit mapping past the visible typo rows");
    }
    require(![panel candidateForDigit:9], "only nine numbered rows are visible");
    if (!count) {
      require(![panel selectedTypoCandidate] && !panel.visible &&
                  ![panel moveTypoSelectionForward:YES client:client],
              "empty typo response is not selectable or navigable");
      continue;
    }
    require(!panel.canBecomeKeyWindow && !panel.canBecomeMainWindow &&
                (panel.styleMask & NSWindowStyleMaskNonactivatingPanel),
            "navigable typo panel stays nonactivating");
    for (int selected = 0; selected < count; ++selected) {
      require([panel selectedTypoCandidate] == selected &&
                  [panel candidateForDigit:std::min(selected, 8)] == selected,
              "every retained correction is reachable with a visible digit");
      require([panel moveTypoSelectionForward:YES client:client],
              "move through the complete retained typo response");
    }
    require([panel moveTypoSelectionForward:YES client:client] &&
                [panel selectedTypoCandidate] == count - 1,
            "typo selection does not wrap after the last candidate");
    for (int i = 0; i < count + 2; ++i)
      require([panel moveTypoSelectionForward:NO client:client],
              "backward typo navigation remains available");
    require([panel selectedTypoCandidate] == 0 &&
                [panel candidateForDigit:0] == 0,
            "backward navigation reveals the first row and clamps at zero");
    [panel reset];
    require(![panel selectedTypoCandidate] && ![panel candidateForDigit:0] &&
                ![panel moveTypoSelectionForward:YES client:client] &&
                !panel.visible,
            "reset discards the cached response, selection and digit mappings");
  }

  const auto response = typoCandidates(12);
  [panel showTypos:response client:client];
  for (int i = 0; i < 11; ++i)
    [panel moveTypoSelectionForward:YES client:client];
  beankey::v1::StateResponse state;
  state.set_candidate_window(beankey::v1::CANDIDATE_WINDOW_SELECTING);
  state.set_selected_candidate(0);
  auto *candidate = state.add_candidates();
  candidate->set_text("通常候補");
  candidate->set_index(42);
  [panel showState:state client:client];
  require(![panel selectedTypoCandidate] &&
              [panel candidateForDigit:0] == 42 &&
              ![panel moveTypoSelectionForward:YES client:client],
          "normal response clears typo mode and its shifted window origin");
  [panel showTypos:response client:client];
  [panel moveTypoSelectionForward:YES client:client];
  [panel showTypos:typoCandidates(0) client:client];
  require(![panel selectedTypoCandidate] && ![panel candidateForDigit:0] &&
              !panel.visible,
          "empty response replaces a previous nonempty typo list");
  [panel reset];
}

static void realDaemonTest(NSString *socketPath) {
  BKDaemonConnection *connection =
      [[BKDaemonConnection alloc] initWithSocketPath:socketPath
                                            launcher:^{
                                            }];
  waitReady(connection);
  BKInputSession *session =
      [[BKInputSession alloc] initWithConnection:connection];
  BKTestTextView *view =
      [[BKTestTextView alloc] initWithFrame:NSMakeRect(0, 0, 500, 300)];
  id<IMKTextInput> client = (id<IMKTextInput>)view;
  std::vector<double> latency;
  [session activate];
  for (int repeat = 0; repeat < 5; ++repeat) {
    for (NSString *letter in @[ @"n", @"i", @"h", @"o", @"n" ]) {
      const auto start = std::chrono::steady_clock::now();
      require([session handleEvent:key(kVK_ANSI_A, letter) client:client],
              "real daemon consumes roman input");
      latency.push_back(std::chrono::duration<double, std::milli>(
                            std::chrono::steady_clock::now() - start)
                            .count());
    }
    require(view.hasMarkedText, "real marked text");
    require([session handleEvent:key(kVK_Space, @" ") client:client],
            "real candidate selection");
    require([session handleEvent:key(kVK_Return, @"\r") client:client],
            "real commit");
    // Enter can commit only the selected clause; finish any remaining clauses
    // through the same lifecycle API used when switching input methods.
    [session commit:client];
    require(!view.hasMarkedText && view.string.length > 0,
            "real committed text");
  }
  [session deactivate:client];
  [connection disconnect];
  std::sort(latency.begin(), latency.end());
  std::cout << "input milliseconds p50=" << latency[latency.size() / 2]
            << " p95=" << latency[(latency.size() * 95) / 100]
            << " max=" << latency.back() << '\n';
  std::cout << "committed=" << view.string.UTF8String << '\n';
}
int main(int argc, const char *argv[]) {
  @autoreleasepool {
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    candidateAnnotationTest();
    candidateTypoWindowTest();
    fakeDaemonTest();
    if (argc == 2)
      realDaemonTest([NSString stringWithUTF8String:argv[1]]);
  }
}
