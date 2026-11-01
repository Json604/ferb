#import <AVFAudio/AVFAudio.h>

NS_ASSUME_NONNULL_BEGIN
// Keep AVFAudio's Objective-C exceptions inside Objective-C frames. Swift catch
// cannot handle them, and unwinding a Swift task corrupts its executor context.
FOUNDATION_EXPORT NSError * _Nullable FERBInstallMicrophoneTap(AVAudioEngine *engine, AVAudioNodeTapBlock block);
FOUNDATION_EXPORT NSError * _Nullable FERBStartMicrophoneEngine(AVAudioEngine *engine);
FOUNDATION_EXPORT NSError * _Nullable FERBStopMicrophoneEngine(AVAudioEngine *engine, BOOL removeTap);
FOUNDATION_EXPORT NSError * _Nullable FERBResetMicrophoneEngine(AVAudioEngine *engine);
NS_ASSUME_NONNULL_END

// Output-only magnetic synthesis. Set is main-thread; Render is a single audio
// consumer. Atomics transfer controls without waiting or allocation in Render.
typedef struct FERBMagneticSynth FERBMagneticSynth;
FERBMagneticSynth * _Nullable FERBMagneticSynthCreate(double sampleRate);
void FERBMagneticSynthDestroy(FERBMagneticSynth * _Nonnull synth);
void FERBMagneticSynthSet(FERBMagneticSynth * _Nonnull synth, float proximity, float gain, float pan, bool fusion);
void FERBMagneticSynthRender(FERBMagneticSynth * _Nonnull synth, float * _Nonnull left, float * _Nonnull right, unsigned int count);

void FERBMagneticSynthCancel(FERBMagneticSynth * _Nonnull synth);
FOUNDATION_EXPORT NSError * _Nullable FERBConfigureMagneticOutput(AVAudioEngine * _Nonnull engine, AVAudioSourceNode * _Nonnull source);
FOUNDATION_EXPORT void FERBDetachMagneticOutput(AVAudioEngine * _Nonnull engine, AVAudioSourceNode * _Nonnull source);

// Output engines (the sound family, magnetic docking): start and stop, and the sound engine's graph and playback.
FOUNDATION_EXPORT NSError * _Nullable FERBStartOutput(AVAudioEngine * _Nonnull engine);
FOUNDATION_EXPORT void FERBStopOutput(AVAudioEngine * _Nonnull engine);
FOUNDATION_EXPORT NSError * _Nullable FERBAttachOutputNode(AVAudioEngine * _Nonnull engine, AVAudioNode * _Nonnull node,
                                                          AVAudioNode * _Nonnull destination, AVAudioFormat * _Nonnull format);
/// Stops the player, schedules the buffer and plays it at `time` (nil: now).
FOUNDATION_EXPORT NSError * _Nullable FERBPlayBuffer(AVAudioPlayerNode * _Nonnull player, AVAudioPCMBuffer * _Nonnull buffer,
                                                    AVAudioTime * _Nullable time);
FOUNDATION_EXPORT void FERBStopPlayer(AVAudioPlayerNode * _Nonnull player);
