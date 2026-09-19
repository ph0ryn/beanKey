#import <Carbon/Carbon.h>
#import <Foundation/Foundation.h>
#include <iostream>

int main(int argc, const char *argv[]) {
  @autoreleasepool {
    if (argc != 2) {
      std::cerr << "usage: beankey-register-input-method BUNDLE\n";
      return 2;
    }
    NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[1]]
                            isDirectory:YES];
    NSBundle *bundle = [NSBundle bundleWithURL:url];
    if (![bundle.bundleIdentifier
            isEqualToString:@"org.ph0ryn.inputmethod.beanKey"]) {
      std::cerr << "not a beanKey input method bundle\n";
      return 2;
    }
    const OSStatus result = TISRegisterInputSource((__bridge CFURLRef)url);
    if (result != noErr) {
      std::cerr << "input source registration failed: " << result << '\n';
      return 1;
    }
    std::cout << "beanKeyを登録しました。システム設定の入力ソースから追加してく"
                 "ださい。\n";
  }
}
