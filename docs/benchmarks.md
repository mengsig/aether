# Harness measurements

These measurements were moved from the README. They describe the named trials,
not a guarantee of parity for other tasks, models, or CLI versions. The
repeatable pre-release comparison is the [release bench](release-bench.md).

## Measured against the native harnesses

On 2026-10-10 and 2026-10-11 two lower-tier models were each run in their
native CLI and through one Aether agent driving that same CLI: same model,
same requested effort (low), same task text, a fresh workspace per trial, and
hidden checks the solver never sees. Two Python repair cases (119 and 154
hidden checks), 10 runs of each per cell in two sittings, 80 trials, with the
same Aether binary, command lines, prompts and CLI versions in both. Each
sitting's runs were fixed before its first and every one is counted whole.
Means per trial:

| Model, effort | Harness | Hidden checks passed | Uncached input + output tokens | Cache read tokens | API-equivalent USD | Wall time | Runs |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Claude Haiku 5.5, low | Claude Code | 99.8 % | 121 k | 2.48 M | 0.3428 | 528 s | 10 |
| Claude Haiku 5.5, low | Aether | 99.6 % | 117 k | 2.05 M | 0.3162 | 490 s | 10 |
| GPT-6 Luna, low | Codex | 73.4 % | 66 k | 0.98 M | 0.0220 | 203 s | 10 |
| GPT-6 Luna, low | Aether | 76.6 % | 93 k | 1.79 M | 0.0348 | 249 s | 10 |

With Haiku, Aether was behind in the mean on hidden checks passed (99.6 %
against 99.8 %; lower in 5 of the 20 pairs, higher in 3, equal in 12). It was
ahead on uncached input tokens (337 against 426, -21 %; lower in 13, higher in
7); output tokens (117 k against 121 k, -3 %; lower in 10, higher in 10);
cache read tokens (2.05 M against 2.48 M, -17 %; lower in 13, higher in 7);
cost (USD 0.3162 against 0.3428, -8 %; lower in 11, higher in 9); wall time
(490 s against 528 s, -7 %; lower in 11, higher in 9). With Luna, Aether was
behind in the mean on uncached input tokens (74 k against 52 k, +43 %; lower
in 6, higher in 14); output tokens (19 k against 14 k, +35 %; lower in 6,
higher in 14); cache read tokens (1.79 M against 984 k, +82 %; lower in 9,
higher in 11); cost (USD 0.0348 against 0.0220, +58 %; lower in 6, higher in
14); wall time (249 s against 203 s, +23 %; lower in 9, higher in 11). It was
ahead on hidden checks passed (76.6 % against 73.4 %; lower in 11, higher in
9).

The 95 % intervals of the mean pair difference, Aether minus native: cost
-0.0868 to +0.0305 USD for Haiku and +0.0015 to +0.0263 USD for Luna; hidden
checks -0.7 to +0.2 points for Haiku and -16.1 to +23.0 points for Luna. Five
Luna trials (3 in Codex, 2 through Aether) were graded 0 because the solution
hung in the grader, and are counted as 0; graded test by test, which is not
the counted figure, Luna passed 85.5 % through Aether against 86.6 % in Codex.
Per case, Luna passed fewer checks through Aether on `gridcalc` (79.5 %
against 81.4 %; lower in 6 of 10 runs, higher in 4) and more as counted on
`matchbook` (73.6 % against 65.4 %; lower in 5 of 10, higher in 5), where all
five hung grades fell and the test-by-test figures are 91.5 % against 91.7 %.
The two sittings point the same way on 7 of these 12 mean differences; they
differ on Haiku's uncached input tokens (-224 then +46), Haiku's cache read
tokens (-976 k then +114 k), Haiku's cost (-0.0701 then +0.0169 USD), Luna's
hidden checks passed (-0.8 then +7.1 points) and Luna's wall time (-10 then
+103 s).

The [first measurement](harness-lower-tier-results-2026-10-10.md), under
protocol version 1 earlier on 2026-10-10, is in none of these figures. After
it Aether stopped repeating the requested effort in words, and both sides of
the Claude pair start with the same MCP set
([diagnosis](harness-overhead-diagnosis-2026-10-10.md)). Haiku's output
was 21 % lower through Aether then, in 9 of 10 pairs, and is 3.5 % lower over
these ten runs, in 10 of 20; separate sittings, not a controlled comparison.

Ten runs per case is still a small sample, and the same cell ranged from 93 to
140 of 154 checks between runs. The intervals and p-values are descriptive:
six measures are named here for each of two models, so one interval that
excludes zero is not by itself a finding, and no significance is claimed.
Those that exclude zero are Luna's uncached input tokens, output tokens and
cost (higher through Aether). 32 of the 40 Haiku trials passed every check, so
accuracy could hardly separate the harnesses for Haiku, and its near-equal
scores are not evidence that nothing is lost. Dollars are API list-price
equivalents, not subscription charges. Wall time was measured with 8 trials at
once and is comparable only within a pair.

A diagnostic sitting of 24 matched Luna pairs on 2026-10-10, none of them
counted, ran the identical Codex command twice in every block as a yardstick:
those two native runs differed by a median of 0.0127 USD (48 % of the native
mean) and by up to 0.0621 USD, beside Luna's counted difference of +0.0127 USD
per trial here. Under a rule written before its first trial, no Aether effect
was claimed there on requests, tokens, cost, time or hidden checks. Its
intervals still allowed one, on cost from 26 % below native to 172 % above and
on hidden checks graded test by test from 19 % below to 2 % above
([diagnosis](harness-overhead-diagnosis-2026-10-10.md#iteration-5)).

Full tables, the statistics, every trial and the limits are in the [report of
2026-10-11](harness-lower-tier-results-2026-10-11.md); the first five
runs also have [their own
report](harness-lower-tier-results-2026-10-10-protocol-2.md). To run it
again (this spends model tokens):

```sh
# One run of 8 trials; the reports repeat it with seeds 1 to 10, each in its own run-<n> directory.
python3 evals/harness_bench.py --cases matchbook,gridcalc --runs 1 --seed 1 --jobs 8 --timeout 1200 \
  --conditions native-haiku-low,aether-single-haiku-low,native-luna-low,aether-single-luna-low \
  --binary target/release/aether --output-dir target/harness-393-v2/run-1
python3 evals/harness_bench_compare.py target/harness-393-v2/run-*/harness-bench-*.json --trials
```

### Six cases at medium effort

A second benchmark runs the same two models at medium effort on six other
repair cases (28 to 102 hidden checks), three runs per case and harness, 72
trials and 18 pairs per model, measured on 2026-10-11 under protocol version
2; means per trial:

| Model | Harness | Mean accuracy | Full passes | Total tokens | API-equivalent USD | Wall time |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Claude Haiku 5.5 | Claude Code | 99.6 % | 13/18 | 1,268,569 | 0.0678 | 253 s |
| Claude Haiku 5.5 | Aether | 99.6 % | 12/18 | 927,132 | 0.0479 | 221 s |
| GPT-6 Luna | Codex | 93.7 % | 2/18 | 326,765 | 0.0088 | 86 s |
| GPT-6 Luna | Aether | 95.4 % | 1/18 | 340,450 | 0.0098 | 140 s |

With Luna, Aether was behind in the mean on total tokens (340,450 against
326,765, +4.2 %), uncached input tokens (36,265 against 28,150, +28.8 %),
output tokens (6,358 against 6,078, +4.6 %), cache read tokens (297,828
against 292,537, +1.8 %), cost (USD 0.0098 against 0.0088, +11.4 %), wall time
(140 s against 86 s, +62.6 %, with one Aether trial that timed out at 900 s)
and full passes (1 against 2 of 18); it was ahead on mean accuracy (95.4 %
against 93.7 %; lower in 7 of the 18 pairs, higher in 7, equal in 4; 95 %
interval of the mean pair difference -1.4 to +5.3 points). With Haiku, Aether
was behind on full passes (12 against 13 of 18) and by 0.1 points on mean
accuracy (99.6 % on both sides after rounding; lower in 2 of the 18 pairs,
higher in 1, equal in 15), and ahead on total tokens (927,132 against
1,268,569, -26.9 %), cost (USD 0.0479 against 0.0678, -29.4 %) and wall time
(221 s against 253 s, -12.6 %); 25 of the 36 Haiku trials passed every check,
so its near-equal scores are not evidence that nothing is lost. Three runs per
case is a small sample and the intervals are descriptive: those that exclude
zero are Haiku's tokens and cost (lower through Aether) and Luna's uncached
input tokens (higher through Aether). An earlier sitting of the same 72
trials, on 2026-10-10 under protocol version 1 and an older Aether binary,
agrees for Haiku in the direction of every measure and differs for Luna, which
then used 23.9 % fewer tokens, cost 14.6 % less and scored 4.3 points lower
through Aether; the two sittings are not pooled, and no difference between
them is attributed to one change. Full tables, the statistics, every trial and
the limits are in the [report of this sitting](harness-bench-2026-10-11.md)
and the [report of the first](harness-bench-2026-10-10.md).
