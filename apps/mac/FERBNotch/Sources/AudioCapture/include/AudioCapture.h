#import <AVFAudio/AVFAudio.h>

NS_ASSUME_NONNULL_BEGIN
// Keep AVFAudio's Objective-C exceptions inside Objective-C frames. Swift catch
// cannot handle them, and unwinding a Swift task corrupts its executor context.
FOUNDATION_EXPORT NSError * _Nullable FERBInstallMicrophoneTap(AVAudioEngine *engine, AVAudioNodeTapBlock block);
FOUNDATION_EXPORT NSError * _Nullable FERBStartMicrophoneEngine(AVAudioEngine *engine);
FOUNDATION_EXPORT NSError * _Nullable FERBStopMicrophoneEngine(AVAudioEngine *engine, BOOL removeTap);
FOUNDATION_EXPORT NSError * _Nullable FERBResetMicrophoneEngine(AVAudioEngine *engine);
NS_ASSUME_NONNULL_END

// The sound family's output engine: start and stop, its graph and playback.
FOUNDATION_EXPORT NSError * _Nullable FERBStartOutput(AVAudioEngine * _Nonnull engine);
FOUNDATION_EXPORT void FERBStopOutput(AVAudioEngine * _Nonnull engine);
FOUNDATION_EXPORT NSError * _Nullable FERBAttachOutputNode(AVAudioEngine * _Nonnull engine, AVAudioNode * _Nonnull node,
                                                          AVAudioNode * _Nonnull destination, AVAudioFormat * _Nonnull format);
/// Stops the player, schedules the buffer and plays it at `time` (nil: now).
FOUNDATION_EXPORT NSError * _Nullable FERBPlayBuffer(AVAudioPlayerNode * _Nonnull player, AVAudioPCMBuffer * _Nonnull buffer,
                                                    AVAudioTime * _Nullable time);
FOUNDATION_EXPORT void FERBStopPlayer(AVAudioPlayerNode * _Nonnull player);
