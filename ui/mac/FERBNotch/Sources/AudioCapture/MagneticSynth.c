#include <stdatomic.h>
#include <stdlib.h>
#include <math.h>
#include <stdbool.h>
_Static_assert(__atomic_always_lock_free(sizeof(float), 0), "Audio controls require lock-free float atomics");
_Static_assert(ATOMIC_INT_LOCK_FREE == 2, "Audio events require lock-free integers");
// C-only translation unit: the render path has no Objective-C/Swift ownership.
typedef struct FERBMagneticSynth {
    _Atomic float proximity, gain, pan;
    _Atomic unsigned int fusion, cancellation, cancelledFusion;
    unsigned int seenFusion, seenCancellation;
    double rate, phase, modPhase, humPhase, tailPhase;
    float smoothProximity, smoothGain, smoothPan;
    double tailTime, tailGain, tailTarget;
} FERBMagneticSynth;
FERBMagneticSynth *FERBMagneticSynthCreate(double rate) {
    if (!isfinite(rate) || rate <= 0) return NULL;
    FERBMagneticSynth *s = calloc(1, sizeof(*s));
    if (s) { s->rate = rate; s->tailTime = 1; }
    return s;
}
void FERBMagneticSynthDestroy(FERBMagneticSynth *s) { free(s); }
void FERBMagneticSynthSet(FERBMagneticSynth *s, float proximity, float gain, float pan, bool fusion) {
    atomic_store_explicit(&s->proximity, fmaxf(0, fminf(1, proximity)), memory_order_relaxed);
    atomic_store_explicit(&s->gain, fmaxf(0, fminf(1, gain)), memory_order_relaxed);
    atomic_store_explicit(&s->pan, fmaxf(-1, fminf(1, pan)), memory_order_relaxed);
    if (fusion) atomic_fetch_add_explicit(&s->fusion, 1, memory_order_release);
}
void FERBMagneticSynthCancel(FERBMagneticSynth *s) {
    atomic_store_explicit(&s->proximity, 0, memory_order_relaxed);
    atomic_store_explicit(&s->gain, 0, memory_order_relaxed);
    atomic_store_explicit(&s->cancelledFusion, atomic_load_explicit(&s->fusion, memory_order_relaxed), memory_order_relaxed);
    atomic_fetch_add_explicit(&s->cancellation, 1, memory_order_release);
}
static double advance(double phase, double frequency, double rate) {
    phase += 6.283185307179586 * frequency / rate;
    return phase - floor(phase / 6.283185307179586) * 6.283185307179586;
}
void FERBMagneticSynthRender(FERBMagneticSynth *s, float *left, float *right, unsigned int count) {
    float p = atomic_load_explicit(&s->proximity, memory_order_relaxed);
    float gain = atomic_load_explicit(&s->gain, memory_order_relaxed);
    float pan = atomic_load_explicit(&s->pan, memory_order_relaxed);
    unsigned int fusion = atomic_load_explicit(&s->fusion, memory_order_acquire);
    unsigned int cancellation = atomic_load_explicit(&s->cancellation, memory_order_acquire);
    if (cancellation != s->seenCancellation) {
        s->seenCancellation = cancellation;
        // Fade any previous resonance as a continuous oscillator, discard its
        // completion envelope so re-grabbing cannot replay a contact.
        s->tailTarget = 0;
    }
    unsigned int cancelledFusion = atomic_load_explicit(&s->cancelledFusion, memory_order_relaxed);
    if (fusion != s->seenFusion) {
        s->seenFusion = fusion;
        if (fusion != cancelledFusion) { s->tailTime = 0; s->tailTarget = 1; s->tailGain = 1; }
    }
    double smoothing = 1 - exp(-1 / (s->rate * .018));
    for (unsigned int i = 0; i < count; ++i) {
        s->smoothProximity += (p - s->smoothProximity) * smoothing;
        s->smoothGain += (gain - s->smoothGain) * smoothing;
        s->smoothPan += (pan - s->smoothPan) * smoothing;
        if (p == 0 && s->smoothProximity < 0.000001) s->smoothProximity = 0;
        if (gain == 0 && s->smoothGain < 0.000001) s->smoothGain = 0;
        double q = s->smoothProximity;
        s->modPhase = advance(s->modPhase, 2.2 + 7*q, s->rate);
        s->phase = advance(s->phase, 145 + 170*q + sin(s->modPhase)*(2 + 10*q), s->rate);
        s->humPhase = advance(s->humPhase, 61 + 17*q, s->rate);
        double signal = q * (.12*sin(s->humPhase) + .2*sin(s->phase)*(0.9+.1*sin(s->modPhase)));
        s->tailGain += (s->tailTarget - s->tailGain) * smoothing;
        if (s->tailTime < .18) {
            double t = s->tailTime / .18;
            s->tailPhase = advance(s->tailPhase, 90 + 310*exp(-t*6), s->rate);
            double envelope = sin(3.141592653589793*t)*exp(-t*4);
            // A brief soft harmonic contact, not a noise zap or sample.
            signal += s->tailGain * (.48*envelope*sin(s->tailPhase) + .045*sin(s->tailPhase*4)*exp(-t*55)*sin(3.141592653589793*fmin(1,t*12)));
            s->tailTime += 1/s->rate;
        }
        signal *= s->smoothGain;
        left[i] = signal * sqrt(.5 - .18*s->smoothPan);
        right[i] = signal * sqrt(.5 + .18*s->smoothPan);
    }
}
