#include "input_controller.h"
#include <cstdlib>
#include <iostream>

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
    require(menu.numberOfItems == 3, "Menu must include the GitHub link");
    require([[menu itemAtIndex:1] isSeparatorItem],
            "GitHub link must be separated from learning reset");
    NSMenuItem *github = [menu itemAtIndex:2];
    require([github.title isEqualToString:@"GitHub"], "GitHub menu label");
    require(github.enabled && github.target == controller &&
                github.action == @selector(openGitHub:),
            "GitHub menu action");
    NSURL *url = github.representedObject;
    require([url.absoluteString
                isEqualToString:@"https://github.com/ph0ryn/beanKey"],
            "GitHub menu destination");
  }
}
