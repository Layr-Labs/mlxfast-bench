# Implementation dependency graph

Auto-generated from the GitHub issue graph. Every edge below is also a **native
GitHub "blocked by" relationship** on the issues themselves (visible in each issue's
sidebar and dependency summary), so the graph here and the tracker stay in sync.

- **48 tasks**, **53 dependency edges**, acyclic: **True**.
- Node label: `WS<id>` and the owning repo + issue number (`bench`/`eng`/`cuda`).
- Arrows point **from a prerequisite to the work it unblocks**.

## Graph

```mermaid
flowchart LR
  subgraph m0["M0 Freeze"]
    WS0_1["WS0-1<br/>bench#2"]
    WS0_2["WS0-2<br/>bench#3"]
    WS0_3["WS0-3<br/>bench#4"]
    WS0_4["WS0-4<br/>bench#5"]
    WS0_5["WS0-5<br/>bench#6"]
    WS0_6["WS0-6<br/>bench#7"]
  end
  subgraph m1["M1 benchd parity"]
    WS1_1["WS1-1<br/>bench#9"]
    WS1_10["WS1-10<br/>bench#18"]
    WS1_2["WS1-2<br/>bench#10"]
    WS1_3["WS1-3<br/>bench#11"]
    WS1_4["WS1-4<br/>bench#12"]
    WS1_5["WS1-5<br/>bench#13"]
    WS1_6["WS1-6<br/>bench#14"]
    WS1_7["WS1-7<br/>bench#15"]
    WS1_8["WS1-8<br/>bench#16"]
    WS1_9["WS1-9<br/>bench#17"]
  end
  subgraph m2["M2 Metal"]
    WS2_1["WS2-1<br/>eng#2"]
    WS2_2["WS2-2<br/>eng#3"]
    WS2_3["WS2-3<br/>eng#4"]
    WS2_4["WS2-4<br/>eng#5"]
    WS2_5["WS2-5<br/>eng#6"]
    WS2_6["WS2-6<br/>eng#7"]
    WS2_7["WS2-7<br/>eng#8"]
  end
  subgraph m3["M3 CUDA correctness"]
    WS3_0["WS3-0<br/>cuda#2"]
    WS3_1["WS3-1<br/>cuda#3"]
    WS3_2["WS3-2<br/>cuda#4"]
    WS3_3["WS3-3<br/>cuda#5"]
    WS3_4["WS3-4<br/>cuda#6"]
    WS3_5["WS3-5<br/>cuda#7"]
    WS3_6["WS3-6<br/>cuda#8"]
  end
  subgraph m4["M4 CUDA perf"]
    WS4_1["WS4-1<br/>cuda#10"]
    WS4_2["WS4-2<br/>cuda#11"]
    WS4_3["WS4-3<br/>cuda#12"]
    WS4_4["WS4-4<br/>cuda#13"]
    WS4_5["WS4-5<br/>cuda#14"]
    WS4_6["WS4-6<br/>cuda#15"]
  end
  subgraph m5["M5 Security"]
    WS5_1["WS5-1<br/>bench#20"]
    WS5_2["WS5-2<br/>bench#21"]
    WS5_3["WS5-3<br/>bench#22"]
    WS5_4["WS5-4<br/>bench#23"]
    WS5_5["WS5-5<br/>bench#24"]
    WS5_6["WS5-6<br/>bench#25"]
    WS5_7["WS5-7<br/>bench#26"]
  end
  subgraph m6["M6 Yukon"]
    WS6_1["WS6-1<br/>bench#28"]
    WS6_2["WS6-2<br/>bench#29"]
    WS6_3["WS6-3<br/>bench#30"]
    WS6_4["WS6-4<br/>bench#31"]
    WS6_5["WS6-5<br/>bench#32"]
  end
  WS0_1 --> WS0_2
  WS0_1 --> WS0_6
  WS0_2 --> WS1_1
  WS0_4 --> WS1_4
  WS1_1 --> WS1_5
  WS1_5 --> WS1_6
  WS1_5 --> WS1_7
  WS1_5 --> WS1_8
  WS1_2 --> WS1_8
  WS1_4 --> WS1_8
  WS1_3 --> WS1_9
  WS1_5 --> WS1_9
  WS1_8 --> WS1_10
  WS0_3 --> WS2_1
  WS0_5 --> WS2_1
  WS2_1 --> WS2_2
  WS1_6 --> WS2_3
  WS2_2 --> WS2_3
  WS2_3 --> WS2_4
  WS1_6 --> WS2_5
  WS2_6 --> WS2_5
  WS2_4 --> WS2_7
  WS2_5 --> WS2_7
  WS0_2 --> WS3_1
  WS3_0 --> WS3_1
  WS3_1 --> WS3_3
  WS3_2 --> WS3_3
  WS3_3 --> WS3_4
  WS3_3 --> WS3_5
  WS3_4 --> WS3_6
  WS3_5 --> WS3_6
  WS3_2 --> WS4_1
  WS3_3 --> WS4_2
  WS3_1 --> WS4_3
  WS3_0 --> WS4_4
  WS4_2 --> WS4_5
  WS4_4 --> WS4_5
  WS4_5 --> WS4_6
  WS3_0 --> WS5_1
  WS5_1 --> WS5_2
  WS1_8 --> WS5_3
  WS5_1 --> WS5_4
  WS5_1 --> WS5_5
  WS5_3 --> WS5_5
  WS1_10 --> WS5_7
  WS2_7 --> WS5_7
  WS3_6 --> WS5_7
  WS4_6 --> WS5_7
  WS5_3 --> WS6_1
  WS3_0 --> WS6_2
  WS6_1 --> WS6_3
  WS6_1 --> WS6_4
  WS6_3 --> WS6_4
```

## Critical path (longest prerequisite chain)

`WS0-1 -> WS0-2 -> WS1-1 -> WS1-5 -> WS1-6 -> WS2-3 -> WS2-4 -> WS2-7 -> WS5-7`

## Roots — startable now (no prerequisites)

WS0-1, WS0-3, WS0-4, WS0-5, WS1-2, WS1-3, WS2-6, WS3-0, WS3-2, WS5-6, WS6-5

## Notes for reviewers

- Some tasks are filed in their **epic's** repo but the code lands elsewhere; the
  issue body's **Where:** line records the true code location (e.g. `bench-transform`
  and `bench-*` tasks under the WS2/WS3/WS4 epics live in `mlxfast-bench`).
- Cross-repo blocking edges are applied natively where the API allows; any that the
  API rejects are recorded as a **Blocked by (cross-repo)** line in the issue body.
