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

static NSEvent *key(unsigned short code, NSString *text) {
  return [NSEvent keyEventWithType:NSEventTypeKeyDown
                          location:NSZeroPoint
                     modifierFlags:0
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
  std::thread server([listener] {
    const int socket = accept(listener, nullptr, nullptr);
    std::map<std::string, std::string> sessions;
    v1::Envelope request;
    while (readFrame(socket, request)) {
      v1::Envelope response;
      response.set_protocol_version(1);
      response.set_request_id(request.request_id());
      response.set_session_id(request.session_id());
      auto *state = response.mutable_state_response();
      state->set_consumed(true);
      state->set_selected_candidate(-1);
      state->set_candidate_window(v1::CANDIDATE_WINDOW_HIDDEN);
      auto &preedit = sessions[request.session_id()];
      if (request.has_key_event() && request.key_event().text() == "x") {
        response.set_request_id(request.request_id() +
                                1); // must reset, not apply
      } else if (request.has_select_candidate() ||
                 request.has_commit_composition()) {
        state->set_commit(preedit);
        preedit.clear();
      } else if (request.has_key_event()) {
        preedit = "仮名😀";
        state->set_candidate_window(v1::CANDIDATE_WINDOW_SELECTING);
        state->set_selected_candidate(0);
        auto *candidate = state->add_candidates();
        candidate->set_text(preedit);
        candidate->set_index(0);
      }
      state->set_preedit(preedit);
      state->set_preedit_cursor(preedit.empty() ? 0 : 3);
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
          "new composition");
  require(![session handleEvent:key(kVK_ANSI_X, @"x") client:client],
          "mismatched request must return the key");
  require([view.string isEqualToString:@"仮名😀"] && !view.hasMarkedText,
          "failure only clears the active marked text");
  require(!connection.ready, "mismatched response disconnects transport");
  [session deactivate:client];
  server.join();
  unlink(path.c_str());
  rmdir(pattern.data());
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
    fakeDaemonTest();
    if (argc == 2)
      realDaemonTest([NSString stringWithUTF8String:argv[1]]);
  }
}
