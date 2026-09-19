#pragma once
#include "daemon_connection.h"
#import <InputMethodKit/InputMethodKit.h>

@interface BKInputSession : NSObject
- (instancetype)initWithConnection:(BKDaemonConnection *)connection;
- (void)activate;
- (BOOL)handleEvent:(NSEvent *)event client:(id<IMKTextInput>)client;
- (void)commit:(id<IMKTextInput>)client;
- (void)deactivate:(id<IMKTextInput>)client;
- (void)resetLearning;
@property(nonatomic, readonly) BOOL learningAvailable;
@end
