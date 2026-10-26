#include "daemon_connection.h"
#include <cstdlib>
#include <iostream>
#include <unistd.h>

@interface BKDaemonConnection (ConfigPathTest)
+ (NSString *)configurationPathForSupport:(NSString *)support;
+ (NSURL *)daemonURLForBundle:(NSBundle *)bundle;
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
    NSString *support = [[NSString stringWithUTF8String:directory]
        stringByResolvingSymlinksInPath];
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
    NSString *original =
        [support stringByAppendingPathComponent:@"beanKey.app"];
    NSString *contents = [original stringByAppendingPathComponent:@"Contents"];
    NSString *resources =
        [contents stringByAppendingPathComponent:@"Resources"];
    require([files createDirectoryAtPath:resources
            withIntermediateDirectories:YES
                             attributes:nil
                                  error:nil],
            "Create synthetic app resources");
    require([@{@"CFBundleIdentifier" : @"test.beankey.daemon-path",
               @"CFBundlePackageType" : @"APPL"}
                writeToFile:[contents stringByAppendingPathComponent:@"Info.plist"]
                 atomically:YES],
            "Create synthetic app metadata");
    NSBundle *originalBundle = [NSBundle bundleWithPath:original];
    require(originalBundle != nil, "Open synthetic app bundle");
    NSURL *originalURL = [BKDaemonConnection daemonURLForBundle:originalBundle];
    NSString *moved =
        [support stringByAppendingPathComponent:@"moved beanKey 日本語.app"];
    require([files moveItemAtPath:original toPath:moved error:nil],
            "Move synthetic app into a path with spaces and non-ASCII");
    NSString *daemonDirectory = [moved stringByAppendingPathComponent:
        @"Contents/Resources/beankey/bin"];
    require([files createDirectoryAtPath:daemonDirectory
            withIntermediateDirectories:YES
                             attributes:nil
                                  error:nil] &&
                [files createFileAtPath:[daemonDirectory
                    stringByAppendingPathComponent:@"beankey-daemon"]
                               contents:[NSData data]
                             attributes:nil],
            "Create bundled daemon after relocation");
    NSBundle *movedBundle = [NSBundle bundleWithPath:moved];
    require(movedBundle != nil, "Open moved synthetic app bundle");
    NSURL *movedURL = [BKDaemonConnection daemonURLForBundle:movedBundle];
    NSString *override = @BEANKEY_DAEMON_PATH;
    if (override.length) {
      require([originalURL.path isEqualToString:override] &&
                  [movedURL.path isEqualToString:override],
              "Explicit daemon path is unchanged by bundle relocation");
    } else {
      require([originalURL.path isEqualToString:
                  [original stringByAppendingPathComponent:
                      @"Contents/Resources/beankey/bin/beankey-daemon"]] &&
                  [movedURL.path isEqualToString:
                      [moved stringByAppendingPathComponent:
                          @"Contents/Resources/beankey/bin/beankey-daemon"]],
              "Daemon path follows relocated bundle resources without PATH lookup");
    }
    require([files removeItemAtPath:support error:nil],
            "Remove temporary support directory");
  }
}
