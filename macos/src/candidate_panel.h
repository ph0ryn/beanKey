#pragma once
#include "beankey.pb.h"
#import <InputMethodKit/InputMethodKit.h>
#include <optional>

@interface BKCandidatePanel : NSPanel
@property(nonatomic, copy) void (^selectCandidate)(uint32_t index);
@property(nonatomic, copy) void (^forgetCandidate)(uint32_t index);
@property(nonatomic, copy) void (^requestTypoCorrections)(void);
- (void)showState:(const beankey::v1::StateResponse &)state
           client:(id<IMKTextInput>)client;
- (void)showTypos:(const beankey::v1::TypoCorrectionResponse &)typos
           client:(id<IMKTextInput>)client;
- (std::optional<uint32_t>)candidateForDigit:(NSUInteger)digit;
- (void)reset;
@end
