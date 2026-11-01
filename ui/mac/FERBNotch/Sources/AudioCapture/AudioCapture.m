#import "AudioCapture.h"

static NSError *CaptureError(void) {
    return [NSError errorWithDomain:@"FERBAudioCapture" code:1 userInfo:@{
        NSLocalizedDescriptionKey: @"The microphone changed or became unavailable. Try dictation again."
    }];
}

NSError *FERBInstallMicrophoneTap(AVAudioEngine *engine, AVAudioNodeTapBlock block) {
    AVAudioInputNode *input = nil;
    @try {
        input = engine.inputNode;
        // Input scope reflects hardware. Output scope can still contain the old
        // 44.1 kHz format while a Bluetooth microphone switches to 16 kHz.
        AVAudioFormat *format = [input inputFormatForBus:0];
        if (format.sampleRate <= 0 || format.channelCount == 0) return CaptureError();
        [input installTapOnBus:0 bufferSize:1024 format:format block:block];
        return nil;
    } @catch (NSException *exception) {
        // A tap may have been partially installed before AVFAudio rejected it.
        @try { [input removeTapOnBus:0]; } @catch (NSException *cleanupException) {}
        return CaptureError();
    }
}

NSError *FERBStartMicrophoneEngine(AVAudioEngine *engine) {
    @try {
        [engine prepare];
        NSError *error = nil;
        if (![engine startAndReturnError:&error]) return error ?: CaptureError();
        return nil;
    } @catch (NSException *exception) { return CaptureError(); }
}

NSError *FERBStopMicrophoneEngine(AVAudioEngine *engine, BOOL removeTap) {
    NSError *error = nil;
    @try { [engine stop]; } @catch (NSException *exception) { error = CaptureError(); }
    // Still remove our tap if stopping throws during a device change.
    if (removeTap) {
        @try { [engine.inputNode removeTapOnBus:0]; }
        @catch (NSException *exception) { error = CaptureError(); }
    }
    return error;
}

NSError *FERBResetMicrophoneEngine(AVAudioEngine *engine) {
    @try { [engine reset]; return nil; }
    @catch (NSException *exception) { return CaptureError(); }
}

static NSError *MagneticOutputError(void) {
    return [NSError errorWithDomain:@"FERBMagneticOutput" code:1 userInfo:@{
        NSLocalizedDescriptionKey: @"Magnetic docking audio output is unavailable."
    }];
}
NSError *FERBConfigureMagneticOutput(AVAudioEngine *engine, AVAudioSourceNode *source) {
    @try {
        [engine attachNode:source];
        [engine connect:source to:engine.mainMixerNode format:[[AVAudioFormat alloc] initStandardFormatWithSampleRate:48000 channels:2]];
        return nil;
    } @catch (NSException *exception) { return MagneticOutputError(); }
}
void FERBDetachMagneticOutput(AVAudioEngine *engine, AVAudioSourceNode *source) {
    @try { [engine disconnectNodeOutput:source]; [engine detachNode:source]; } @catch (NSException *exception) {}
}

static NSError *OutputError(void) {
    return [NSError errorWithDomain:@"FERBOutput" code:1 userInfo:@{
        NSLocalizedDescriptionKey: @"Audio output is unavailable."
    }];
}
NSError *FERBStartOutput(AVAudioEngine *engine) {
    @try {
        NSError *error = nil;
        if (![engine startAndReturnError:&error]) return error ?: OutputError();
        return nil;
    } @catch (NSException *exception) { return OutputError(); }
}
void FERBStopOutput(AVAudioEngine *engine) {
    @try { [engine stop]; } @catch (NSException *exception) {}
}
NSError *FERBAttachOutputNode(AVAudioEngine *engine, AVAudioNode *node, AVAudioNode *destination, AVAudioFormat *format) {
    @try {
        if (node.engine == nil) [engine attachNode:node];
        [engine connect:node to:destination format:format];
        return nil;
    } @catch (NSException *exception) { return OutputError(); }
}
NSError *FERBPlayBuffer(AVAudioPlayerNode *player, AVAudioPCMBuffer *buffer, AVAudioTime *time) {
    @try {
        [player stop];
        [player scheduleBuffer:buffer atTime:nil options:0 completionHandler:nil];
        [player playAtTime:time];
        return nil;
    } @catch (NSException *exception) { return OutputError(); }
}
void FERBStopPlayer(AVAudioPlayerNode *player) {
    @try { [player stop]; } @catch (NSException *exception) {}
}
