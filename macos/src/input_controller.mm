#include "input_controller.h"
#include "input_session.h"

@implementation BKInputController {
  BKInputSession *_session;
}
- (id)initWithServer:(IMKServer *)server
            delegate:(id)delegate
              client:(id)client {
  self = [super initWithServer:server delegate:delegate client:client];
  if (self)
    _session = [[BKInputSession alloc]
        initWithConnection:BKDaemonConnection.sharedConnection];
  return self;
}
- (NSUInteger)recognizedEvents:(id)sender {
  (void)sender;
  return NSEventMaskKeyDown;
}
- (void)activateServer:(id)sender {
  // No synchronous client text queries during activation (notably Chromium).
  [sender overrideKeyboardWithKeyboardNamed:@"com.apple.keylayout.US"];
  [_session activate];
}
- (BOOL)handleEvent:(NSEvent *)event client:(id)sender {
  return [_session handleEvent:event client:sender];
}
- (void)commitComposition:(id)sender {
  [_session commit:sender];
}
- (void)deactivateServer:(id)sender {
  [_session deactivate:sender];
}
- (void)inputControllerWillClose {
  [_session deactivate:self.client];
}
- (NSMenu *)menu {
  NSMenu *menu = [[NSMenu alloc] initWithTitle:@"beanKey"];
  NSMenuItem *reset = [menu addItemWithTitle:@"学習データを消去"
                                      action:@selector(resetLearning:)
                               keyEquivalent:@""];
  reset.target = self;
  reset.enabled = _session.learningAvailable;
  menu.autoenablesItems = NO;
  return menu;
}
- (void)resetLearning:(id)sender {
  (void)sender;
  [_session resetLearning];
}
@end
