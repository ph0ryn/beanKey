#include "daemon_connection.h"
#include "input_controller.h"
#import <InputMethodKit/InputMethodKit.h>

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    // Keep the controller class linked even though InputMethodKit instantiates
    // it by name.
    [BKInputController class];
    NSBundle *bundle = NSBundle.mainBundle;
    // ARC must retain the server throughout the event loop, even in release
    // builds.
    __attribute__((objc_precise_lifetime))
    IMKServer *server = [[IMKServer alloc]
            initWithName:
                [bundle objectForInfoDictionaryKey:@"InputMethodConnectionName"]
        bundleIdentifier:bundle.bundleIdentifier];
    if (!server) {
      NSLog(@"beanKey: could not create the InputMethodKit server");
      return 1;
    }
    [BKDaemonConnection.sharedConnection prepare];
    [NSApp run];
  }
}
