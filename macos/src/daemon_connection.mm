#include "daemon_connection.h"
#include <memory>
#include <sys/stat.h>

@implementation BKDaemonConnection {
  std::shared_ptr<beankey::Client> _client;
  NSString *_socketPath;
  void (^_launcher)(void);
  BOOL _preparing;
  NSUInteger _generation;
}
+ (instancetype)sharedConnection {
  static BKDaemonConnection *connection;
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    NSString *runtime = [NSHomeDirectory()
        stringByAppendingPathComponent:@"Library/Caches/beanKey/runtime"];
    NSString *learning =
        [NSHomeDirectory() stringByAppendingPathComponent:
                               @"Library/Application Support/beanKey/learning"];
    connection = [[self alloc]
        initWithSocketPath:
            [runtime stringByAppendingPathComponent:@"beankey/daemon.sock"]
                  launcher:^{
                    NSError *error = nil;
                    if (![[NSFileManager defaultManager]
                                  createDirectoryAtPath:runtime
                            withIntermediateDirectories:YES
                                             attributes:@{
                                               NSFilePosixPermissions : @0700
                                             }
                                                  error:&error]) {
                      NSLog(@"beanKey: cannot create runtime directory: %@",
                            error);
                      return;
                    }
                    NSTask *task = [[NSTask alloc] init];
                    task.executableURL =
                        [NSURL fileURLWithPath:@BEANKEY_DAEMON_PATH];
                    task.arguments = @[
                      @"--config", @BEANKEY_CONFIG_PATH, @"--runtime-root",
                      runtime, @"--learning-directory", learning
                    ];
                    NSString *logs = [NSHomeDirectory()
                        stringByAppendingPathComponent:@"Library/Logs/beanKey"];
                    if (![[NSFileManager defaultManager]
                                  createDirectoryAtPath:logs
                            withIntermediateDirectories:YES
                                             attributes:@{
                                               NSFilePosixPermissions : @0700
                                             }
                                                  error:&error]) {
                      NSLog(@"beanKey: cannot create log directory: %@", error);
                      return;
                    }
                    NSString *logPath =
                        [logs stringByAppendingPathComponent:@"daemon.log"];
                    if (![[NSFileManager defaultManager]
                            fileExistsAtPath:logPath] &&
                        ![[NSFileManager defaultManager]
                            createFileAtPath:logPath
                                    contents:nil
                                  attributes:@{
                                    NSFilePosixPermissions : @0600
                                  }]) {
                      NSLog(@"beanKey: cannot create daemon log");
                      return;
                    }
                    NSFileHandle *log =
                        [NSFileHandle fileHandleForWritingAtPath:logPath];
                    if (!log) {
                      NSLog(@"beanKey: cannot open daemon log");
                      return;
                    }
                    [log seekToEndOfFile];
                    task.standardOutput = log;
                    task.standardError = log;
                    if (![task launchAndReturnError:&error])
                      NSLog(@"beanKey: daemon launch failed: %@", error);
                    [log closeFile];
                  }];
  });
  return connection;
}
- (instancetype)initWithSocketPath:(NSString *)path
                          launcher:(void (^)(void))launcher {
  self = [super init];
  if (self) {
    _socketPath = [path copy];
    _launcher = [launcher copy];
  }
  return self;
}
- (BOOL)ready {
  return _client && _client->connected();
}
- (NSUInteger)generation {
  return _generation;
}
- (void)prepare {
  NSAssert([NSThread isMainThread], @"IPC composition is main-thread confined");
  if (self.ready || _preparing)
    return;
  _preparing = YES;
  const auto client = std::make_shared<beankey::Client>(_socketPath.UTF8String);
  void (^launch)(void) = _launcher;
  const NSUInteger generation = _generation;
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    const bool connected = client->ensureConnected([launch] { launch(); },
                                                   std::chrono::seconds(30));
    dispatch_async(dispatch_get_main_queue(), ^{
      self->_preparing = NO;
      if (generation != self->_generation)
        return;
      if (connected)
        self->_client = client;
      else
        NSLog(@"beanKey: daemon was not ready within 30 seconds");
    });
  });
}
- (std::optional<beankey::v1::Envelope>)request:
    (const beankey::v1::Envelope &)request {
  NSAssert([NSThread isMainThread], @"IPC requests are main-thread confined");
  if (!self.ready)
    return std::nullopt;
  auto response = _client->request(request, std::chrono::seconds(5));
  if (!response || response->protocol_version() != request.protocol_version() ||
      response->request_id() != request.request_id() ||
      response->session_id() != request.session_id() ||
      (!response->has_state_response() &&
       !response->has_typo_correction_response())) {
    [self disconnect];
    return std::nullopt;
  }
  return response;
}
- (void)disconnect {
  _client.reset();
  ++_generation;
}
@end
