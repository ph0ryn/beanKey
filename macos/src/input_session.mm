#include "input_session.h"
#include "candidate_panel.h"
#include "key_event.h"
#include "text_input.h"
#include <string>

using namespace beankey;

@implementation BKInputSession {
  BKDaemonConnection *_connection;
  BKCandidatePanel *_panel;
  id<IMKTextInput> _textClient;
  std::string _sessionId;
  uint64_t _nextRequest;
  NSUInteger _generation;
  v1::StateResponse _state;
  BOOL _typos;
}
- (instancetype)initWithConnection:(BKDaemonConnection *)connection {
  self = [super init];
  if (self) {
    _connection = connection;
    _panel = [[BKCandidatePanel alloc] init];
    __weak BKInputSession *weakSelf = self;
    _panel.selectCandidate = ^(uint32_t index) {
      [weakSelf select:index];
    };
    _panel.forgetCandidate = ^(uint32_t index) {
      [weakSelf forget:index];
    };
    _panel.requestTypoCorrections = ^{
      [weakSelf requestTypos];
    };
  }
  return self;
}
- (void)activate {
  [_panel reset];
  [_connection prepare];
}
- (BOOL)learningAvailable {
  return _state.learning_available();
}
- (v1::Envelope)envelope {
  v1::Envelope request;
  request.set_protocol_version(1);
  request.set_request_id(_nextRequest++);
  request.set_session_id(_sessionId);
  return request;
}
- (void)clear {
  if (_textClient && !_state.preedit().empty()) {
    [_textClient setMarkedText:@""
                selectionRange:NSMakeRange(0, 0)
              replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
  }
  _state.Clear();
  _sessionId.clear();
  _typos = NO;
  [_panel reset];
}
- (void)fail {
  [_connection disconnect];
  [self clear];
}
- (BOOL)start {
  if (_generation != _connection.generation)
    [self clear];
  if (!_sessionId.empty())
    return YES;
  if (!_connection.ready) {
    [_connection prepare];
    return NO;
  }
  _generation = _connection.generation;
  _sessionId = NSUUID.UUID.UUIDString.UTF8String;
  _nextRequest = 1;
  auto request = [self envelope];
  *request.mutable_start_session()->mutable_surrounding_text() =
      macos::surroundingText(_textClient);
  const auto response = [_connection request:request];
  if (!response || !response->has_state_response()) {
    [self fail];
    return NO;
  }
  _state = response->state_response();
  return YES;
}
- (int)cursorMovement:(uint32_t)index {
  for (const auto &candidate : _state.candidates())
    if (candidate.index() == index) {
      int movement = 0;
      for (const auto &action : candidate.actions())
        movement += action.move();
      return movement;
    }
  return 0;
}
- (BOOL)send:(const v1::Envelope &)request movement:(int)movement {
  const auto response = [_connection request:request];
  if (!response || !response->has_state_response()) {
    [self fail];
    return NO;
  }
  const auto &state = response->state_response();
  if (!macos::applyText(state, _textClient, movement,
                        !_state.preedit().empty())) {
    [self fail];
    return NO;
  }
  _state = state;
  _typos = NO;
  [_panel showState:_state client:_textClient];
  return state.consumed();
}
- (BOOL)select:(uint32_t)index {
  if (!_textClient || ![self start])
    return NO;
  auto request = [self envelope];
  if (_typos)
    request.mutable_select_typo_correction()->set_index(index);
  else
    request.mutable_select_candidate()->set_index(index);
  return [self send:request movement:_typos ? 0 : [self cursorMovement:index]];
}
- (void)forget:(uint32_t)index {
  if (!_textClient || ![self start])
    return;
  auto request = [self envelope];
  request.mutable_forget_candidate()->set_index(index);
  [self send:request movement:0];
}
- (void)requestTypos {
  if (!_textClient || ![self start])
    return;
  auto request = [self envelope];
  request.mutable_request_typo_corrections();
  const auto response = [_connection request:request];
  if (!response || !response->has_typo_correction_response()) {
    [self fail];
    return;
  }
  if (response->typo_correction_response().candidates_size()) {
    _typos = YES;
    [_panel showTypos:response->typo_correction_response() client:_textClient];
  }
}
- (void)resetLearning {
  if (!_textClient || ![self start])
    return;
  auto request = [self envelope];
  request.mutable_reset_learning();
  [self send:request movement:0];
}
- (BOOL)handleEvent:(NSEvent *)event client:(id<IMKTextInput>)client {
  if (event.type != NSEventTypeKeyDown)
    return NO;
  _textClient = client;
  auto key = macos::keyEvent(event);
  if (key.action() == v1::USER_ACTION_UNSPECIFIED) {
    if (event.modifierFlags & NSEventModifierFlagCommand)
      [self commit:client];
    return NO;
  }
  if (![self start]) {
    [self clear];
    return NO;
  }
  const auto forbidden = NSEventModifierFlagCommand |
                         NSEventModifierFlagControl | NSEventModifierFlagOption;
  NSString *text = event.characters;
  if (!(event.modifierFlags & forbidden) && text.length == 1 &&
      [text characterAtIndex:0] >= '1' && [text characterAtIndex:0] <= '9') {
    const auto index =
        [_panel candidateForDigit:[text characterAtIndex:0] - '1'];
    if (index)
      return [self select:*index];
  }
  *key.mutable_surrounding_text() = macos::surroundingText(client);
  auto request = [self envelope];
  *request.mutable_key_event() = key;
  const int movement = _state.selected_candidate() >= 0
                           ? [self cursorMovement:_state.selected_candidate()]
                           : 0;
  return [self send:request movement:movement];
}
- (void)commit:(id<IMKTextInput>)client {
  _textClient = client;
  if (_sessionId.empty() || _generation != _connection.generation) {
    [self clear];
    return;
  }
  auto request = [self envelope];
  request.mutable_commit_composition();
  [self send:request movement:0];
}
- (void)deactivate:(id<IMKTextInput>)client {
  _textClient = client;
  [self commit:client];
  if (!_sessionId.empty() && _connection.ready) {
    auto request = [self envelope];
    request.mutable_end_session();
    [_connection request:request];
  }
  [self clear];
  _textClient = nil;
}
@end
