#include "input_controller.h"
#import <AppKit/NSWorkspace.h>
#include <cstdlib>
#include <iostream>
#import <objc/runtime.h>

static NSURL *openedURL;

static BOOL captureOpenURL(id workspace, SEL selector, NSURL *url) {
  (void)workspace;
  (void)selector;
  openedURL = url;
  return YES;
}

static void require(bool value, const char *message) {
  if (!value) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}

int main() {
  @autoreleasepool {
    BKInputController *controller = [BKInputController new];
    NSMenu *menu = [controller menu];
    NSMenuItem *github = [menu itemAtIndex:menu.numberOfItems - 1];
    require([github.title isEqualToString:@"GitHub"], "GitHub menu label");
    require(github.enabled && github.target == controller &&
                github.action == @selector(openGitHub:),
            "GitHub menu action");
    Method openURL =
        class_getInstanceMethod(NSWorkspace.class, @selector(openURL:));
    require(openURL != nullptr, "NSWorkspace openURL: is available");
    IMP original = method_setImplementation(openURL, (IMP)captureOpenURL);
    [controller doCommandBySelector:github.action
                  commandDictionary:@{kIMKCommandMenuItemName : github}];
    method_setImplementation(openURL, original);
    require([openedURL.absoluteString
                isEqualToString:@"https://github.com/ph0ryn/beanKey"],
            "GitHub menu destination");
    require(menu.numberOfItems == 2, "Menu must contain only two commands");
  }
}
