#include "daemon_connection.h"
#include <cstdlib>
#include <iostream>
#include <unistd.h>

@interface BKDaemonConnection (ConfigPathTest)
+ (NSString *)configurationPathForSupport:(NSString *)support;
@end

void require(bool value, const char *message) {
  if (!value) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}

int main() {
  @autoreleasepool {
    char directory[] = "/tmp/beankey-config-test.XXXXXX";
    require(mkdtemp(directory) != nullptr,
            "Create temporary support directory");
    NSString *support = [NSString stringWithUTF8String:directory];
    NSString *configured =
        [support stringByAppendingPathComponent:@"config.toml"];
    NSFileManager *files = [NSFileManager defaultManager];
    require([BKDaemonConnection configurationPathForSupport:support] == nil,
            "Missing configuration fails before installation");
    require([@"default" writeToFile:configured
                        atomically:YES
                          encoding:NSUTF8StringEncoding
                           error:nil],
            "Create generated default configuration");
    require([[BKDaemonConnection configurationPathForSupport:support]
                isEqualToString:configured],
            "Installed configuration is used");
    require([files removeItemAtPath:configured error:nil],
            "Remove user configuration");
    require(symlink("missing.toml", configured.fileSystemRepresentation) == 0,
            "Create dangling user configuration link");
    require([BKDaemonConnection configurationPathForSupport:support] == nil,
            "Dangling user configuration fails instead of silently resetting");
    require([files removeItemAtPath:support error:nil],
            "Remove temporary support directory");
  }
}
