#pragma once
#include "client.h"
#import <Foundation/Foundation.h>

@interface BKDaemonConnection : NSObject
+ (instancetype)sharedConnection;
- (instancetype)initWithSocketPath:(NSString *)path
                          launcher:(void (^)(void))launcher;
- (void)prepare;
- (std::optional<beankey::v1::Envelope>)request:
    (const beankey::v1::Envelope &)request;
- (void)disconnect;
@property(nonatomic, readonly) BOOL ready;
@property(nonatomic, readonly) NSUInteger generation;
@end
