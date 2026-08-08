# 目录/标题模式完整清单（Pattern Catalog）

> 目的：让"一种模式 = 一种算法"的分类有据可依。每个模式类对应一个匹配器
> （matcher），用户可通过 `--toc-mode` 指定；所有模式按"锚定"策略工作：
> 先找「目录/Contents」标记页（或书签），页内条目几乎全是目录，误报≈0；
> 无目录页时才用模式扫正文标题（此时各模式必须保守）。

## 分级约定

- `level 0`：部/卷级（卷、编、部、册、集、辑、篇、部分、单元、Part、Volume、Book、Unit）
- `level 1`：章级（章、回、话、讲、专题、模块、幕、折、出、Chapter、Lesson、Lecture、Act、Module、Topic、Canto、Appendix、Annex、Article、Schedule、Exhibit）
- `level 2`：节级（节、课、场、则、條、条、Section、Scene、Clause、1.1、A.1）
- `level 3`：条/款/项/目级（款、项、目、（一）、（1）、1.1.1、(a)）

---

## A. 中文编号体系

### A1. 「第X + 量词」序数词（cjk-unit）

格式：`第 <数字> <量词> <标题>`；数字可带空格（`第 一 章`）。

| 量词 | 层级 | 说明 | 现状 |
|---|---|---|---|
| 卷、编、部、册、集、辑、篇、部分、单元 | 0 | 部/卷级 | ✅ 已实现 |
| 章、回、话、讲、专题、模块、幕、折、出 | 1 | 章级；回=章回体，幕/折/出=戏曲 | ✅ 已实现 |
| 节、课、场、则、條、条 | 2 | 节级；場=场景 | ✅ 已实现 |
| 款、项、目 | 3 | 法律条文之下 | ✅ 已实现 |

数字形式：阿拉伯 `1`、全角 `１`、中文小写 `一二三…`（已支持）、
**中文大写 `壹贰叁肆伍陆柒捌玖拾`（财务/法律，缺失）**、
罗马 `I`（少见）、天干 `甲`（极罕见）。

### A2. 不带「第」的卷式（classical / 古籍）

- `卷一` `卷二` `卷十`（汉字数字）、`卷1`、`卷之X`（`卷之一`）、`卷上/中/下`
- `上卷/中卷/下卷`、`上册/中册/下册`、`上篇/中篇/下篇`
- 例：《滹南集》"卷一 五经辨惑"、《類篇》"卷之一"
- ✅ 已实现（卷一/卷之X/卷上中下/上中下册/上中下篇；裸"上册"需带标题才匹配）

### A3. 公文层次序数（cjk-seq，GB/T 9704-2012）

官方四级序数：`一、` → `（一）` → `1.` → `（1）`，附 ⑤ `①`。

| 形式 | 层级 | 现状 |
|---|---|---|
| `一、` `二、` `三、`（汉字+顿号）；`甲、乙、`（天干） | 1 | ✅ 已实现 |
| `（一）` `（二）`（全角/半角括号） | 2 | ✅ 已实现 |
| `1.` `2.`（见 B2/A8，归属 decimal） | 3 | 部分 |
| `（1）` `(1)`、`①` `②` `③`、`(a)` | 3 | ✅ 已实现 |
| 变体：`1、` `1）` | 3 | 未实现（`1.` 归 decimal） |

### A4. 法律条文（legal）

结构：编 → 章 → 节 → 条 → 款 → 项 → 目。
- `第X編/章/節/條`：✅ 全部支持；條 在完整法典中严格应为 level 3，当前按 2（单行法典无章时更自然）。
- 款：无编号（自然段），不参与目录。
- 项：`（一）（二）`（见 A3）；目：`1. 2.`（见 decimal）。
- 例：中華民國憲法"第一章 總綱"（含空格 `第 一 章`，heading 扫描**曾失败**，已修复）。

### A5. 章回体小说（zhanghui）

- `第一回` `第二回`…（回目常为两句对仗）；`楔子` `引子` `缘起` `入话`（前置标记）
- 回 = level 1；✅ 已实现（已列入 A1）。

### A6. 戏曲（drama）

- `第X幕/场/折/出/齣`；幕=1、场=2、折/出=1；✅ 已实现（已列入 A1）。

### A7. 标准/规范（standards）

- `附录A` `附录B`、`附錄A`（繁体）、`A.1` `A.1.1`（Annex 内章节）、`第X部分`（已支持）
- 附录字母编号（`Appendix A`/`Annex A`/`附录A`）✅ 已实现；`A.1` ✅ 已实现（letter-decimal）。

### A8. 科技文献编号（CY/T 35-2001）

正文/目次中章编号**不加"第"、不加"章"**：`1`、`1.1`、`1.1.1`。
- `1.1 背景`（decimal）已支持（含中文标题）。
- `1 引言`（单层 + 空格 + 中文标题）——TOC 语境与正文 heading 扫描均已支持（数字+中文标题）。

### A9. 中文前后置文关键词（keyword）

- 前置：目录、目次、序、序言、前言、自序、引言、绪论、绪言、导言、导论、凡例、例言、致谢
- 后置：附录、附錄、参考文献、参考书目、索引、后记、後記、跋、结语
- ✅ 已扩展：目录/目次/附录/附錄/参考文献/参考书目/后记/後記/前言/序言/序/引言/绪论/绪言/导言/导论/结语/跋/凡例/例言/致谢/致謝。

### A10. 极罕见（仅记录，不实现）

天干地支作序号（`甲、乙、`）、千字文序号（`天地玄黄…`）、`卷第一`、`章之X`。

---

## B. 英文编号体系

### B1. Label + Number（en-label）

| 标签 | 层级 | 现状 |
|---|---|---|
| Part、Volume、Book、Unit | 0 | ✅ 已实现 |
| Chapter、Lesson、Lecture、Act、Module、Topic、Canto | 1 | ✅ 已实现 |
| Section、Scene、Clause | 2 | ✅ 已实现 |
| Appendix、Annex、Article、Schedule、Exhibit | 1 | ✅ 已实现 |
| 数字形式 | — | 现状 |
| Arabic `Chapter 1` | | Chapter ✓ |
| Roman `Chapter IV`、`Part II` | | ✅ 已实现 |
| Word `Chapter One`、`Part Two` | | ✅ 已实现 |
| Letter `Appendix A`、`Exhibit B`、`Annex C` | | ✅ 已实现 |
| Decimal `Section 1.2` | | ✅ 已实现 |
| 分隔符 `:` `-` `.` `—` `\|` | | ✅ 已实现 |

### B2. Decimal 数字编号（decimal，Chicago/Turabian/技术书）

- `1`、`1.1`、`1.1.1`、`1.1.1.1` → 层级=点数（已支持，封顶 3）
- 字母后缀 `1.1(a)`、`§1.1`、`1.1: Title` → **缺失/部分**（`:` 分隔未支持）
- 中文标题 `1 引言` / `1.1 背景` → 部分（见 A8）

### B3. 大纲/字母编号（outline / letter-decimal）

- `I. Title`、`II. Title`（大写罗马+句点）✅ 已实现
- `A. Title`、`A.1 Title`、`A.1.1`（Annex/Exhibit 编号）✅ 已实现
- `(a) Title`、`(1) Title`（法律/合同/大纲）✅ 已实现（（1）归入 A3 cjk-seq）
- `a.`/`i.`（小写字母/小写罗马大纲）未实现（误报风险高，仅记录）

### B4. 英文前后置文关键词（keyword）

- 前置：Foreword、Preface、Introduction、Acknowledgements、Abstract、
  Executive Summary、List of Figures、List of Tables、List of Illustrations、
  Acronyms、Abbreviations、Notation、Prologue、Dedication
- 后置：Conclusion、References、Bibliography、Appendix、Glossary、Index、
  Epilogue、Afterword、Postscript、Colophon
- IMRAD：Introduction / Methods / Results / Discussion / Conclusion / Abstract
- ✅ 已扩展：新增 Executive Summary、List of Figures/Tables/Illustrations、
  Acronyms、Abbreviations、Notation、Prologue、Epilogue、Afterword、
  Postscript、Dedication、Colophon、Acknowledgement。

### B5. 戏剧/诗歌（drama/poetry）

- Act/Scene（见 B1）、Prologue、Epilogue、Interlude
- Canto（诗章）、Book I/II（史诗卷，见 B1）
- 现状：Act/Scene/Canto ✅ 已实现（已列入 B1）。

### B6. 英文法律（legal-EN）

- Article、§、Section、Subsection、Clause、Paragraph、Schedule、Annex
- `§ 1-107`（章-部分-条，连字符）→ 极罕见，仅记录

---

## C. 目录页布局/页码模式（structure）

| 模式 | 说明 | 现状 |
|---|---|---|
| 点线引导 dot-leader | `Title……42`（`.` `·` `…` `⋯`） | ✓ |
| 右对齐页码 | 末尾空白+数字 | ✓ |
| 前置页码 | `43第6课…` | ✓ |
| 多栏网格 | 一行多组"标题+页码" | ✓ |
| 缩进层级 | 缩进→层级 | ✓（layout） |
| `p.42` / `Page 42` / `第42页` / `42页` | 带前缀/后缀的页码 | ✅ 已实现 |
| 罗马数字页码 | 前辅文 i, ii, iii | 记录，不实现 |

---

## D. 其他语言（记录）

- 日语：`第X章/節/話/巻/編/話`（与 A1 字符相同，自动覆盖）；目次=目录标记
- 韩语：`제X장/절/과`（Hangul，超出 CJK 匹配范围，不实现）
- 越南/泰/阿拉伯等：不实现

---

## 模式 → 算法映射（建议 --toc-mode 值）

| mode | 对应模式类 | 误报风险 |
|---|---|---|
| `cjk-unit` | A1 + A2 + A4（第X量词、卷X、条条款项目） | 极低 |
| `cjk-seq` | A3（公文 一、 （一） 1. （1） ①） | 低 |
| `en-label` | B1 + B5（Label+数字，含罗马/单词/字母） | 低 |
| `decimal` | A8 + B2（N.N.N） | 中高→强约束 |
| `letter-decimal` | B3（I. A. A.1 (a)） | 中 |
| `dot-leader` | C 点线行 | 极低 |
| `allcaps` | 全大写（INTRODUCTION、CHAPTER ONE） | 中 |
| `keyword` | A9 + B4（目录/前言/References/…） | 低 |
| `classical` | A2 古籍卷式（卷一/卷之一/上中下卷） | 极低 |

锚定优先：`outlines`（ISO 32000-1 §12.3.3）→ 目录标记页（任何模式）→
正文模式扫描（用户指定或 auto 顺序）。

---

## 实现状态（2026-08）

以上 ✅ 项均已并入 `src/toc.rs` 的匹配器（`match_cjk_heading`、`match_cjk_seq`、
`match_classical_volume`、`match_en_label`、`match_chapter`/`match_part`（罗马/单词数字）、
`match_letter_numbered`、`level_from_keywords`），通过 `try_chapter_heading` /
`level_from_title` 统一进入 auto 链（plain → layout → heading），并用
`--toc-mode plain|layout|heading` 仍可强制单算法。每个模式类仍是独立的匹配器，
正文扫描默认保守（拒绝句末标点、无标题的裸序号、纯测量行等）；用户可按
`--toc-modes` 查看并强制某一路径。

尚未实现（仅记录，避免误报）：`1、`/`1）` 变体、`a.`/`i.` 小写大纲、`§` 前缀、
`1.1(a)` 字母后缀、罗马/天干/千字文作中文序号、韩语 제X장。
