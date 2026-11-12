# Apolo9 — circuit reconstruction of the SOMA Lyra-8

This is the engineering record behind the DSP: what each stage of the Lyra-8
is, which component values it uses, how sure we are, and how the model
implements it.

## Sources and evidence levels

There is **no public schematic** of the Lyra-8. The ModWiggler DIY thread
(where builders discuss the circuit) is not reachable by automated fetch, and
the DIY kit ships PCB artwork rather than schematics. The reconstruction
combines:

| Tag | Meaning |
|---|---|
| **[PCB]** | Read from the v2.6 MAIN board silkscreen (`reference/L8 DIY v3.3/PCBs/MAIN board.tif`) or the v2.6 BOM in the DIY assembly manual. |
| **[TWEAK]** | From SOMA's annotated tweaks sheet (`Tweaks.jpg`), which names the role of specific parts. |
| **[DOC]** | Stated in the user manual v2.0/v2.1 or SOMA's FAQ (Vlad Kreimer). |
| **[INFER]** | Topology inferred from the above plus datasheets. The PCB images show only the top copper layer, so not every net can be traced. |
| **[ART]** | Where information runs out, a decision made for musical/organismic reasons. |

Main documentary facts used:

* Voices are HEF4093 Schmitt-trigger oscillators "used for their analog
  properties", taking a **triangle output** rather than the usual square;
  the square is used for SHARP. **[DOC]**
* "The envelope generators are part of the oscillators and have an impact on
  their pitch and waveform … it is also a kind of waveshaper." There is no
  VCA at the end of the chain, so part of the circuit is always open. **[DOC]**
* Significant crosstalk: silent oscillators bleed, especially at high
  tunings. **[DOC]**
* The delay is digital only in its memory (PT2399), with an analog clock
  modulation input, analog feedback loop, filters and two-tap mixer. It is
  lo-fi and noisy at long times. **[DOC]**
* "Sub harsh noise (enharmonic oscillations)" can appear at high PITCH. **[DOC]**

## Chip inventory **[PCB]**

| Part | Qty | Role |
|---|---|---|
| 4093 NXP (SOIC) | 2 | voice oscillators: one gate per voice, voices 1-4 / 5-8 per package |
| 4093 TI (DIP) | 3 | 2 × vibrato bank (8 gates), 1 × Hyper LFO ("4093-BEAT") |
| PT2399 | 2 | delay lines |
| TL074 | 3 | pair/group mixers, delay feedback/mix, distortion + output |
| TDA2822 | 1 | headphone amplifier ("6V-PH") |
| BC557 | 8 (+2) | one per voice: sensor amplifier / envelope charge pump |
| BC547 | 6 (+1) | rails, buffers |
| 78L05 | 2 | 5 V for the PT2399s |
| 4016 | 1 (IN/OUT) | jack-insert switching of the CV inputs |

## 1. Voice oscillator (×8)

**Circuit.** A 4093 NAND with one input tied high is an inverting Schmitt
trigger. Its output drives the timing capacitor through a 100 Ω series
resistor ("IC-x 100om" pads) **[PCB]** and the TUNE pot; the capacitor
voltage is the voice's "triangle". **[DOC][INFER]**

**Timing capacitors** (tweaks sheet positions 1-8, "can be 0.05-10 µF, the
less the cap the higher the voice and the more crazy FM") **[TWEAK][PCB]**:

| Voice | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|---|---|---|
| C | 2.2 µF el. | 2.2 µF el. | 1 µF el. | 1 µF el. | 0.47 µF film | 0.47 µF film | 0.22 µF film | 0.22 µF film |

This explains the manual's "voices 1-2 bass, 7-8 twice as high as 5-6".

**Model** (`src/dsp/schmitt.rs`):

* HEF4093B thresholds vs supply: centre ≈ 0.47·VDD, hysteresis ≈ 0.25 +
  0.125·VDD. At 12 V that gives VT+ ≈ 6.5 V and VT− ≈ 4.8 V. Both have a
  per-package and per-gate spread. **[INFER: datasheet typicals]**
* The output on-resistance (≈ 250 Ω at 12 V) sits in series with the pot.
  At the top of the TUNE range it is a large part of the timing resistance,
  so very high tunings drift sharp and become unstable. **[INFER]**
* **Exact integration.** Between threshold crossings the node is a linear
  RC network, so it is advanced with the closed-form exponential. Each
  crossing is located in closed form (log) at sub-sample precision, and
  several crossings per sample are handled. The square output gets a
  polyBLEP residual. A unit test checks the period against
  `T = RC·[ln((VDD−VT−)/(VDD−VT+)) + ln(VT+/VT−)]` to within 0.2 %.
* Everything else tied to the node is a **Norton equivalent** recomputed
  every sample:
  * FM current;
  * vibrato current;
  * envelope-dependent loading;
  * electrolytic leakage (25 MΩ on voices 1-4);
  * dielectric absorption: a 3 % "ghost" capacitor behind 2 MΩ on the
    electrolytics, which gives the low voices a slight memory of their
    recent history.

Measured range on serial 424 (TUNE 0 → 1, PITCH near max): V1 8 Hz → 2 kHz,
V5 32 Hz → 8 kHz, V7 70 Hz → 18 kHz. This matches the manual's "from tens to
thousands of hertz".

**TUNE** is a C100k pot wired so clockwise lowers the resistance (reverse
log). **[PCB: BOM][INFER]**
**PITCH** scales the charge current of the whole group, which preserves the
intervals (manual: "transposes … preserving the intervals; close to maximum
is the normal position"). The exact transistor topology is not traceable, so
it is modelled as a group-wide conductance scale. **[INFER]**

## 2. Sensor and envelope (×8)

Tweaks sheet **[TWEAK]**: GREEN "attack time 300 k – 1 MΩ" (910 k on v2.6),
RED "FAST release 220 k – 750 k" (330 k), BLUE "SLOW release 1 MΩ – infinity"
(3 MΩ). The DIY manual mentions an empty slot for "resistors for decreasing
the sensors' sensitivity, 3-10 MΩ". **[DOC]**

**Reconstruction** **[INFER]**:

```
 +12 ─────────────┬──────────── BC557 emitter
                  │ (empty slot: B-E shunt 3-10 MΩ = less sensitive)
 pad(sensitive) ─ 910k ─ base     collector ─ 5.1k ─┬─ ENV (2.2 µF)
 pad(contact)  ─ 0 V via the skin                   ├─ 3 MΩ  → 0 V   (SLOW)
                                                    ├─ 330k → 0 V   (FAST switch)
 HOLD pot ─ 1N4148 ─ 750k ───────────────────────────┤  (one-way lift)
                                                    └─ 10 V zener (ceiling)
```

* Skin resistance across the pads sets the base current. β (≈ 180-380 per
  unit) times that current charges the 2.2 µF, limited by 5.1 k: about 10 ms
  for a firm touch, seconds for a feather.
* Light pressure gives a slow attack, and a feather only opens the envelope
  partially (equilibrium against the release resistor). This is the manual's
  "apply less pressure, you open the envelope only partially".
* FAST puts 330 k in parallel with 3 MΩ: release τ drops from 6.6 s to
  0.65 s. The same leak makes the sensor need more pressure ("less
  sensitive", **[DOC]** ✓) and divides the HOLD voltage more (0.8 → 0.28),
  so "FAST requires a higher HOLD" falls out of the circuit. **[DOC]** ✓
* HOLD can only lift the envelope (diode), never pull it down: "limited from
  below by the HOLD function". **[DOC]** ✓
* The zener only sinks current above its 10 V knee.
* Model: implicit Euler on the linearised conductances, at host rate
  (`Voice::envelope`).

**Touch input.** Velocity (and poly/channel aftertouch) maps to finger
pressure p. Skin resistance is R = 10^(10.5 − 5.7p): 63 kΩ for a firm, moist
press up to GΩ for a feather. Contact builds up in about 8 ms; moisture
lingers about 25 ms after release. A small physiological tremor (≈ 9 Hz)
modulates the conductance. **[ART]**

**Humidity** (lab control) adds a leakage path across each pad, scaled by
a per-unit spread. Raise it and some voices start to sound on their own, as
the manual warns for dew or rain. **[DOC][ART]**

## 3. The voice "VCA" and waveshaper (×8)

The manual's HOLD GATE text speaks of "the VCA", yet SOMA says there is no
VCA at the end of the chain. Per voice the remaining parts are 200 k, 68 k,
39 k, 5.1 k, 47 µF and the second zener. **[PCB]**

**Reconstruction** **[INFER]**: the envelope biases a junction. The triangle
reaches it through 39 k into 5.1 k (×0.116), and SHARP adds the gate's
square through a pot. The junction current is

`i = bias(ENV) · exp(v_node / V_T,eff)`, saturating at the transistor limit,

with the bias following a forward-biased knee around 1.6 V of envelope. This
gives:

* an expo-style VCA whose **waveshape depends on the envelope**. Low
  envelope means only the positive peaks get through (thin, buzzy); high
  envelope rounds the tops. This is "a kind of waveshaper". **[DOC]** ✓
* even harmonics from the asymmetric exponential;
* a small always-open feed-through, and the envelope's own thump through
  the output coupling (≈ 6 Hz HPF).

**Envelope → pitch.** The junction loads the timing node through a large
resistance whose far end the envelope lifts. Opening a voice therefore
raises its pitch slightly and tilts the triangle, more so at low tunings.
**[DOC]** ✓ "the envelope generators … have an impact on their pitch and
waveform".

## 4. Pair mixers and FM

TL074 summing amps: 68 k in, 75 k ‖ 100 pF feedback (−1.1 gain, 21 kHz
pole) **[PCB]**. They run on a single 12 V rail with a 5.87 V virtual ground
(test point) **[PCB]**, so they clip asymmetrically: about +4.7 V / −4.1 V
around the bias. All op-amp stages use this rail model. Each pair bus is
also that pair's FM source ("FM12 … FM78" pins) **[PCB]**.

* MOD is a dual-gang A100k, one gang per voice of the pair (4 stereo A100k
  in the BOM). **[PCB][INFER]**
* FM is a **current injected into the 4093 timing node**, plus a small pull
  toward the virtual ground. Consequences:
  * The same current is a larger fraction of the charge current at low
    tunings: "the higher a voice is tuned, the less sensitive it is to FM".
    **[DOC]** ✓
  * At high MOD the injected current can exceed the charge current. The
    oscillator then stalls and restarts, and the loop "behaves as an LFO":
    "maximum settings result in self-oscillation". **[DOC]** ✓
  * Because the source is post-VCA, a voice's influence decays with its
    envelope. **[DOC]** ✓
* Routing per manual:
  * Source switch per pair: up = VOICES, centre = off, down = LFO CV.
  * FM structure switch down = two loops (12⇄34, 56⇄78); up = 34→56 and
    78→12, forming one ring.
  * LFO CV is replaced by the output when TOTAL FB is on, and by the CV
    VOICES jack when it is plugged.

## 5. Crosstalk, noise, unit spread **[DOC][INFER][ART]**

* **Ground bounce.** Every output edge of a voice kicks the thresholds of
  the gates in its package (and 30 % into the other package) for about
  1.5 µs. A voice near its threshold at that moment switches early, so
  oscillators tuned close together pull into lock and beat irregularly.
  This is how real CMOS oscillators sharing a die behave.
* **Rail sag.** The summed junction current shifts all thresholds slowly,
  so one voice's envelope breathes into the pitch of the others.
* **Bleed.** The edge currents of the squares leak into the master bus
  through a high-pass, louder for high tunings. **[DOC]** ✓
* **Noise.** Threshold jitter (0.4 mV rms at 192 kHz), a slow random walk
  of each TUNE network (thermal drift), mixer hiss, finger tremor. All are
  scaled by the NOISE lab control.
* **Serial.** Seeds every tolerance:
  * electrolytic timing caps ±20 %, film ±5 %, resistors ±5 %;
  * β of each BC557;
  * thresholds per package and per gate (the "batch" effect SOMA mentions);
  * PT2399 VCO constant ±8 %.

  Unit 424 is the default; every serial number is a different instrument.

## 6. Vibrato bank **[PCB][TWEAK][DOC]**

* Eight 4093 oscillators with 10 µF caps and resistors of 56 k, 51 k, 47 k,
  43 k (voices 1-4) and 39 k, 36 k, 33 k, 30 k (voices 5-8). That gives
  ≈ 2.6–5 Hz, a different rate per voice (manual: "eight independent
  vibrato generators"; tweaks: "8 × individual vibrato freq, 2 k – 72 k").
* They are coupled through the bipolar 10 µF caps (UNI 10 µF ×8) and
  47 k / 24 k.
* The VIBRATO switch **switches the +12 V of those chips**, as builders
  confirmed by probing pin 14. Turning it on boots eight oscillators from
  discharged caps; turning it off lets the supply collapse and the coupling
  caps relax. Both transients are modelled.

## 7. Hyper LFO ("4093-BEAT") **[PCB][DOC]**

* Two 4093 oscillators on 47 µF with 1 k + C100k: ≈ 0.36–36 Hz.
* LINK feeds A's output into B's timing node through 910 k (soft FM).
* AND uses the remaining NAND gates (logic product of the squares). OR is a
  resistive sum, a three-level staircase with "a gradient" **[DOC]**.
* A small RC leaves the edge clicks in: Vlad calls them "the only noise I
  don't like".
* The delay's TRI is the sum of both capacitor voltages ("summation of two
  triangular signals"); the delay's SQR is the AND output.

## 8. MOD DELAY — 2 × PT2399 **[PCB][DOC][INFER]**

* **Clock.** The PT2399 VCO runs at f ≈ 59.6 MHz·k / (R + 2.21 k). RAM
  holds 44 000 bits written at f/15.5, so the delay is ≈ 683/f_MHz ms
  (datasheet relation).
  * TIME = 0.47 k + 100 k log pot: about 31 ms → 1.2 s.
  * The top of the range is beyond the chip's nominal 340 ms ("the circuitry
    allows using the delay far above its normal time"). **[DOC]**
* **The bitstream is simulated.**
  * A second-order sigma-delta writes one bit per bit-clock tick into a real
    44 000-bit ring; the bit read back from the same address is the echo.
  * The read-back bitstream is integrated exactly over each sample
    (continuous-time demodulation, no sampling alias).
  * It then passes the reconstruction filters, including the 1.2 nF the
    tweaks sheet calls "Delay LPF 0-5600 pF".
  * Consequences, as on the hardware: noise rises as the time lengthens
    (the oversampling ratio collapses), there are idle tones, overload
    grinds, and sweeping the time Doppler-shifts the stored audio.
* **Modulation** injects current into the VCO pin: LFO (TRI/SQR), SELF (the
  line's own output, low-passed at 1.5 kHz) or CV DELAY (overrides both when
  plugged, per manual).
* **Cross-feedback.** Line 1 is fed by line 2 and vice versa through a
  TL074 stage with two 1N4148 per polarity (the four diodes by "MIX DRY")
  **[PCB]**. Loop gain passes unity just over the middle of FB **[DOC]**,
  and the diode limit keeps the self-oscillation as bounded standing waves.
  The two-tap mixer sums both lines; MIX crossfades.

## 9. Distortion and output **[PCB][TWEAK][INFER]**

* DRIVE is a dual-gang pot ("DRV1/DRV2") setting two cascaded non-inverting
  TL074 gains.
* The clipping element is the single-rail TL074 itself, which is asymmetric
  and therefore produces even harmonics.
* 510 pF across the 91 k feedback ("distortion brightness 0-0.01 µF")
  **[TWEAK]** rolls off the gained part above ≈ 3.4 kHz.
* The TL07x phase reversal (an input below the common-mode range flips the
  output to the positive rail) is kept for extreme drive.
* Output EQ: a little low boost and softer highs (manual: tuned for live
  PA), a subsonic HP, VOL, a last op-amp, and a divider to the 2 V
  0-to-peak spec **[DOC]**. Peak output stays below 0.95 FS (tested) even at
  absurd settings.
* TOTAL FB taps the output after distortion, before VOL.

## 10. Rates and performance

* Audio-rate circuitry runs at ≥ 176.4 kHz (4× at 44.1/48 kHz, 2× at
  88.2/96 kHz): the 4093 oscillators, junction VCAs, mixers, PT2399
  bitstreams, distortion and output stage.
* Slow physics runs at host rate: envelopes, skin dynamics, vibrato bank,
  Hyper LFO, pot smoothing.
* A 193-tap Kaiser FIR decimates the output (24 samples of latency at 4×,
  reported to the host).
* Cost: about 5 % of one Apple-silicon core at 48 kHz (≈ 19× realtime
  offline), allocation-free on the audio thread (tested).

## Open questions

These are things a schematic or scope measurement of a real unit would
settle:

* the exact PITCH transistor topology (modelled as a charge-current scale);
* the junction-VCA bias network values (200 k / 68 k / 47 µF roles are
  inferred);
* the PT2399's internal modulator order (2nd order chosen to match its
  known noise behaviour);
* the vibrato injection depth per voice, and which two voices use the
  24 k couplings.
