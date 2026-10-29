#import <AVFAudio/AVFAudio.h>

NS_ASSUME_NONNULL_BEGIN
// Keep AVFAudio's Objective-C exceptions inside Objective-C frames. Swift catch
// cannot handle them, and unwinding a Swift task corrupts its executor context.
FOUNDATION_EXPORT NSError * _Nullable FERBInstallMicrophoneTap(AVAudioEngine *engine, AVAudioNodeTapBlock block);
FOUNDATION_EXPORT NSError * _Nullable FERBStartMicrophoneEngine(AVAudioEngine *engine);
FOUNDATION_EXPORT NSError * _Nullable FERBStopMicrophoneEngine(AVAudioEngine *engine, BOOL removeTap);
FOUNDATION_EXPORT NSError * _Nullable FERBResetMicrophoneEngine(AVAudioEngine *engine);
NS_ASSUME_NONNULL_END
