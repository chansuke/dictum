# dictum 設計ドキュメント

2026-11-18

## 概要

dictumは[expr-lang](https://expr-lang.org/)（Go）の構文を引き継いだRust製の式言語で、ルールエンジンへの組み込みを第一の用途とする。ルールは一度コンパイルし、多数の事実データに対して安全かつ高速に繰り返し評価する。

### 目標

- expr-langと同じ構文（リテラル、演算子、述語、pipe、let）を受け付ける
- コンパイル時に静的型チェックを行い、ルールの戻り値型（通常は `bool`）を固定できる
- 副作用がなく、実行上限によって必ず停止する評価（ユーザー定義関数が副作用を持たず停止する限り）
- コンパイル済みルールは `Send + Sync` で、複数スレッドから共有できる
- 「なぜマッチしたか／しなかったか」を返すexplain機能

### 非目標

- Go版との挙動の完全互換（エラーメッセージや数値の細かい規則）
- 汎用スクリプト言語化（ループ、代入、I/O）
- 初期段階でのバイトコードVM

## Go版exprとの差分

構文は同じだが、曖昧な挙動は「コンパイル時に弾く」「実行時に黙って値を変えない」方向に寄せる。

| 項目 | expr-lang（Go） | dictum |
| --- | --- | --- |
| 整数オーバーフロー | 値が一周する | `EvalErrorKind::Overflow` |
| 文字列の添字とスライス | バイト単位。文字の途中でも切れる | バイト単位。文字の途中を指すと `EvalErrorKind::CharBoundary` |
| 存在しないフィールド | 設定次第でnilかエラー | コンパイルエラー |
| nilになりうる値 | どの値もnilになりうる | `Option<T>` 型だけ。`?.` と `??` でのみ扱える |
| `int` と `float` の混在 | 暗黙に変換 | `int` → `float` の昇格だけ。`/` は `int` 同士でも `float`、`%` は `int` のみ |
| 金額 | `float` で扱う | `Decimal` 型（`decimal` feature） |
| 比較の連鎖 `a < b < c` | `a < b && b < c` に展開。`b` を2回評価する | 同じく展開するが、`b` は1回だけ評価する |
| `a == b == c` | `(a == b) == c` | 構文エラー |
| マップの順序 | 不定 | 挿入順（`IndexMap`） |
| `now()` | 実時刻 | 評価コンテキストが渡す時計。テストで固定できる |
| 日時の型 | Goの `time` | `jiff` の `Timestamp` と `SignedDuration` |
| 実行時エラー | `error` と panic が混在 | すべて `Result` |

次の2点は差分と誤解しやすいが、両者で同じである。`len` はどちらも文字数を返し、比較できない型同士の `==` はどちらもコンパイルエラーになる。

## アーキテクチャとクレート構成

コストのかかる処理（パース・型チェック・最適化）はルール登録時に一度だけ行い、評価時はクロージャ木に事実データを渡すだけにする。

```mermaid
flowchart LR
    subgraph compile["コンパイル時（ルール登録時に一度だけ）"]
        SRC["ルール文字列<br/>例: user.Age >= 18"] --> PARSE["Parse<br/>dictum-syntax<br/>出力: AST"]
        PARSE --> CHECK["Check<br/>dictum-check<br/>出力: 型付きIR"]
        CHECK --> COMPILE["Compile<br/>dictum-eval<br/>出力: クロージャ木"]
        DERIVE["#[derive(Dictum)]<br/>dictum-derive"] -- 型情報 --> SCHEMA["Schema<br/>型情報だけを持つ<br/>全ルールで共有"]
        SCHEMA --> CHECK
    end
    subgraph eval["評価時（事実データごとに何度でも）"]
        FACTS["事実データ<br/>derive済みの構造体<br/>ゼロコピーで参照"] -- 入力 --> RUN["rule.eval()<br/>dictum<br/>実行上限つき"]
        RUN --> RESULT["結果<br/>bool などの戻り値<br/>explain() で理由も"]
    end
    COMPILE -- コンパイル済みRule --> RUN
```

上段がコンパイル、下段が評価。`Schema` の型情報と、事実データのフィールドを読み出す実装は、どちらも同じ `#[derive(Dictum)]` から生成される。そのため、derive した型については型情報と実際のデータが食い違わない。

| クレート | 責務 | 主な依存 |
| --- | --- | --- |
| `dictum-syntax` | Lexer、Parser、AST、Span | なし |
| `dictum-check` | `Type`、`Schema`、型付きIR | `dictum-syntax` |
| `dictum-eval` | `Value`、Optimizer、クロージャコンパイル、組み込み関数、実行上限 | `dictum-check`、`jiff`、`regex`、`indexmap` |
| `dictum-derive` | `#[derive(Dictum)]` | `syn`、`quote` |
| `dictum` | 外部向けAPI（`Rule`、`RuleSet`、`explain`） | 上記すべて |

### MSRVと `no_std`

MSRVはRust 1.85。edition 2024 は1.85以上を要求するので、これより下げられない。MSRVを上げるのはマイナーバージョン更新時だけとし、CIで常に1.85をテストする。

全クレートを `#![no_std]` + `alloc` で書き、`std` は既定で有効なfeatureにする。

| 依存 | `no_std` での使い方 |
| --- | --- |
| `regex` | `default-features = false` で `alloc` のみ。`std` がないと内部でスピンロックを使うので、気になる環境では `regex-automata` を直接使う |
| `jiff` | `alloc` のみ。タイムゾーンDBは使えないので、`timezone()` は `std` が必要 |
| `indexmap` | ハッシュ関数を明示する（`foldhash`） |
| `rust_decimal` | `default-features = false` |

エラー型は `core::error::Error` を実装する。`now()` は評価コンテキスト `Ctx` から時計を受け取るので `no_std` でもそのまま動く。`no_std` でのビルドは `thumbv7em-none-eabihf` 向けにCIで確認する。

## 構文仕様

字句と文法は[expr-langの言語定義](https://expr-lang.org/docs/language-definition)に合わせ、優先順位だけdictumとして明文化する。パーサは手書きのPrattパーサ。

### 字句規則

| 種類 | 規則 | 例 |
| --- | --- | --- |
| コメント | `//` 行末まで、`/* */` ブロック | `// note` |
| 識別子 | `[A-Za-z_][A-Za-z0-9_]*` と `$env` | `user`, `$env` |
| 整数 | 10進・`0x`・`0o`・`0b`、`_` 区切り可 | `1_000`, `0x2A` |
| 浮動小数 | 小数点または指数つき | `0.5`, `.5`, `1e3` |
| 文字列 | `"` か `'`（`\n` `\t` `\uXXXX`）、バッククォートは生文字列 | `'admin'` |
| バイト列 | `b"..."`（`\xNN` `\NNN`） | `b"\x00"` |
| 予約語 | `true false nil let if else not and or in matches contains startsWith endsWith` | |
| 述語内の参照 | `#`、`#index`、`#acc`、`.Field` | `.Age > 18` |

### 文法（EBNF）

```
program     = { "let" ident "=" expr ";" } expr ;
expr        = conditional ;
conditional = pipe [ "?" [ expr ] ":" expr ]      (* a ?: b は a ? a : b *)
            | "if" expr "{" expr "}" "else" ( "{" expr "}" | conditional ) ;
pipe        = binary { "|" ident "(" [ args ] ")" } ;
binary      = unary { binop unary } ;        (* 優先順位は下の表 *)
unary       = ( "not" | "!" | "-" | "+" ) unary | postfix ;
postfix     = primary { "." ident | "?." ident | "[" index "]" | "?.[" index "]" | "(" [ args ] ")" } ;
index       = expr | [ expr ] ":" [ expr ] ;
primary     = literal | ident | "$env" | pointer | "." ident
            | "(" expr ")" | "[" [ args ] "]" | "{" [ pairs ] "}" ;
pointer     = "#" [ "index" | "acc" ] ;
args        = arg { "," arg } [ "," ] ;
arg         = expr | "{" expr "}" ;                (* 述語の括弧は省略可 *)
pairs       = key ":" expr { "," key ":" expr } [ "," ] ;
key         = ident | string | "(" expr ")" ;
```

`a | f(b)` はパース時に `f(a, b)` へ脱糖する。述語内の `.Field` は `#.Field` に変換する。

### 演算子の優先順位（低い→高い）

| 順位 | 演算子 | 結合 | Go版の値 |
| --- | --- | --- | --- |
| 1 | `? :`、`?:`、`if else` | 右 | 二項演算子をすべて読んだ後に処理 |
| 2 | `\|`（pipe） | 左 | 0 |
| 3 | `or` `\|\|` | 左 | 10 |
| 4 | `and` `&&` | 左 | 15 |
| 5 | `<` `>` `<=` `>=` | 連鎖 | 20 |
| 5 | `==` `!=` `in` `not in` `matches` `contains` `startsWith` `endsWith` | 非結合 | 20 |
| 6 | `..` | 非結合 | 25 |
| 7 | `+` `-` | 左 | 30 |
| 8 | 単項 `not` `!` | 前置 | 50 |
| 9 | `*` `/` `%` | 左 | 60 |
| 10 | 単項 `-` `+` | 前置 | 90 |
| 11 | `**` `^` | 右 | 100 |
| 12 | `??` | 左 | 500 |
| 13 | `.` `?.` `[]` `[:]` `()` | 後置 | |

順位は[Go版の `operator.go` と `parser.go`](https://github.com/expr-lang/expr/tree/master/parser)（2026年7月のmain）で確認し、すべてGo版に合わせる。

- `not` は `+` より強く `*` より弱い。`not a == b` は `(not a) == b` になる（Rustの `!a == b` と同じ）。
- `??` は他の二項演算子と括弧なしで混ぜられない。`a ?? 0 + 1` はエラーで、`(a ?? 0) + 1` と書く。
- `-2 ** 2` は `-(2 ** 2)`。`a | f() ? x : y` は `f(a) ? x : y`。
- 大小比較は連鎖できる。`18 <= user.Age < 65` は `18 <= user.Age && user.Age < 65` と同じ意味で、短絡評価する。中央の `user.Age` は1回だけ評価し、explainでは比較ごとの子ノードになる。
- `<` と `>` のように向きを混ぜてもよい（Go版と同じ）。`==` `!=` `in` などは連鎖できず、`a == b == c` は構文エラー。

## ASTとSpan

全ノードがソース上のバイト範囲 `Span` を持ち、型エラーも実行時エラーもexplainもこのSpanで位置を示す。識別子はインターンして `Symbol`（`u32`）で持つ。

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span { pub start: u32, pub end: u32 }   // バイトオフセット

pub struct Expr { pub kind: ExprKind, pub span: Span }

pub enum ExprKind {
    // リテラル
    Nil, Bool(bool), Int(i64), Float(f64),
    Str(Box<str>), Bytes(Box<[u8]>),
    Array(Vec<Expr>),
    Map(Vec<(MapKey, Expr)>),
    // 参照
    Ident(Symbol),
    Env,                                   // $env
    Pointer(Pointer),                      // #, #index, #acc
    // 演算
    Unary  { op: UnaryOp,  expr: Box<Expr> },
    Binary { op: BinaryOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Member { obj: Box<Expr>, key: MemberKey, optional: bool },
    Slice  { obj: Box<Expr>, from: Option<Box<Expr>>, to: Option<Box<Expr>> },
    Call   { callee: Symbol, args: Vec<Expr> },
    Predicate(Box<Expr>),                  // all(xs, { ... }) の本体
    // 制御
    Conditional { cond: Box<Expr>, then: Box<Expr>, otherwise: Box<Expr> },
    Let { name: Symbol, value: Box<Expr>, body: Box<Expr> },
    // エラー回復
    Error,                                 // 構文エラーの箇所。Parserが置く
}

pub enum MemberKey { Field(Symbol), Index(Box<Expr>) }
pub enum Pointer { Current, Index, Acc }
```

- pipeと述語内の `.Field` はパーサで脱糖するので、ASTに専用ノードはない。
- `user.Name` と `user["Name"]` はどちらも `Member` になり、キーが定数ならCheckerがフィールド番号に解決する。
- 初期実装は `Box` で組む。パース速度が問題になったらアリーナ化を検討する。

## 型システムとSchema

CheckerはASTと `Schema` から、型を確定しフィールドを番号に解決した型付きIRを作る。型が分からない箇所だけ `Any` にして実行時チェックを挿入する。

```rust
pub enum Type {
    Any, Nil, Bool, Int, Float, String, Bytes, Time, Duration,
    #[cfg(feature = "decimal")]
    Decimal,
    Array(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Option(Box<Type>),
    Struct(Arc<StructType>),        // deriveが生成
    Func(Arc<FuncSig>),
    Error,                          // 型エラー後の置き換え。連鎖エラーを止める
}

pub struct StructType { pub name: &'static str, pub fields: Vec<Field> }
pub struct Field { pub name: &'static str, pub ty: Type }
```

### 型付け規則（主なもの）

| 式 | 規則 | 結果の型 |
| --- | --- | --- |
| `Int + Int` | オーバーフローで実行時エラー | `Int` |
| `Int + Float` | `Int` を `Float` に昇格 | `Float` |
| `a / b` | `Int` と `Float` のどの組み合わせでも | `Float` |
| `Decimal + Int` | `Int` を `Decimal` に昇格 | `Decimal` |
| `Decimal / Decimal` | ゼロ除算と桁あふれは実行時エラー | `Decimal` |
| `Decimal + Float` | コンパイルエラー。`decimal(x)` か `float(d)` で明示する | |
| `String + String` | 連結 | `String` |
| `s[i]`、`s[i:j]` | バイト位置。文字境界でなければ実行時エラー | `String` |
| `Time - Time` | | `Duration` |
| `x.f`（`x: Option<S>`） | コンパイルエラー。`?.` が必要 | |
| `x?.f`（`x: Option<S>`, `f: T`） | | `Option<T>` |
| `a ?? b`（`a: Option<T>`, `b: T`） | | `T` |
| `x in xs`（`xs: Array<T>`） | `x` は `T` と比較可能 | `Bool` |
| `all(xs, p)` | `p` は `#: T` で `Bool` | `Bool` |
| `x ⊕ Any` | 実行時に型を確認 | 演算子による |

`Decimal` は `rust_decimal::Decimal`。構文を変えないため専用リテラルは設けず、事実データのフィールドか `decimal("19.99")` で作る。定数引数の `decimal()` はOptimizerがコンパイル時に値へ畳み込む。

### Schema API

```rust
let schema = Schema::builder()
    .var::<User>("user")                       // #[derive(Dictum)] の型情報
    .var_type("tags", Type::array(Type::String))
    .func("double", |x: i64| x * 2)            // シグネチャは型から推論
    .build();                                  // Arcで共有できる
```

`#[derive(Dictum)]` はフィールドのRust型から `Type` を作る。`String` → `String`、`Vec<T>` → `Array`、`Option<T>` → `Option`、`HashMap<String, T>` → `Map`、入れ子の構造体 → `Struct`。`#[dictum(rename = "Age")]` で式中の名前を変えられる。

## 評価エンジン

型付きIRを型ごとに特化したクロージャの木へ変換し、評価時はその木を呼ぶだけにする。値は事実データを借用する `Value<'a>` で、文字列や構造体をコピーしない。

### Value

```rust
pub enum Value<'a> {
    Nil, Bool(bool), Int(i64), Float(f64),
    #[cfg(feature = "decimal")]
    Decimal(rust_decimal::Decimal),
    Str(Cow<'a, str>),
    Bytes(Cow<'a, [u8]>),
    Time(jiff::Timestamp),
    Duration(jiff::SignedDuration),
    Array(Cow<'a, [Value<'a>]>),
    Map(Arc<IndexMap<Arc<str>, Value<'a>>>),
    Object(&'a dyn Object),                  // derive済みの構造体を借用
}

pub trait Object: Send + Sync {
    fn struct_type(&self) -> &'static StructType;
    fn field(&self, id: FieldId) -> Value<'_>;   // 名前でなく番号で引く
}
```

戻り値を保存したいときは `into_owned()` で `Value<'static>` にする。

### クロージャコンパイル

```rust
type Node = Box<dyn for<'a> Fn(&mut Frame<'a>) -> Result<Value<'a>, EvalError> + Send + Sync>;

pub struct Frame<'a> {
    root: &'a dyn Object,      // 事実データ
    slots: Vec<Value<'a>>,     // let 変数
    pointers: Vec<Value<'a>>,  // #, #index, #acc のスタック
    budget: Budget,
    clock: &'a dyn Clock,
    cache: Vec<Option<Value<'a>>>,  // RuleSet の共通部分式（後述）
}
```

- 型が確定しているので、`Int + Int` などは専用クロージャを生成し、実行時の型分岐を省く。
- `&&` と `||` は短絡評価。述語関数（`all` など）も結果が決まった時点で止める。
- ベンチマークで速度不足が分かったときに、クロージャ木をバイトコードVMに後から差し替える。

### Optimizer（クロージャ生成の前）

- 定数の畳み込み：`60 * 60 * 24` → `86400`
- 定数配列への `in`：ハッシュ集合に置き換える
- `x in 1..100`：配列を作らず範囲チェックにする
- 定数パターンの `matches`：コンパイル時に `Regex` を構築する

### 実行上限

```rust
pub struct Limits {
    pub max_steps: u64,        // コレクションの反復回数の合計
    pub max_alloc_items: u64,  // 生成する配列・文字列の要素数の合計
    pub max_depth: u32,        // 述語の入れ子の深さ
}
```

各ノードで数えると遅いので、反復と確保の箇所だけで数える。`1..n` や `repeat` は確保する前に要素数を予算から引く。超えたら `EvalErrorKind::BudgetExceeded` のエラーを返す。

## 組み込み関数

expr-langの組み込み関数をすべて提供し、ルールでよく使うものをMVPに入れる。各関数はシグネチャを持ち、Checkerが引数の型を検査する。

| カテゴリ | MVP | その後 |
| --- | --- | --- |
| 文字列 | `trim` `upper` `lower` `split` `replace` `hasPrefix` `hasSuffix` `indexOf` | `trimPrefix` `trimSuffix` `splitAfter` `repeat` `lastIndexOf` |
| 数値 | `max` `min` `abs` `ceil` `floor` `round` | |
| 配列の述語 | `all` `any` `none` `one` `filter` `map` `count` `find` `findIndex` | `findLast` `findLastIndex` `groupBy` `reduce` |
| 配列の操作 | `first` `last` `sum` `len` `join` | `concat` `flatten` `uniq` `mean` `median` `take` `reverse` `sort` `sortBy` |
| マップ | `keys` `values` | |
| 型変換 | `int` `float` `string` `type` `decimal` | `toJSON` `fromJSON` `toBase64` `fromBase64` `toPairs` `fromPairs` |
| 日時 | `now` `duration` `date` | `timezone` |
| その他 | `get` | ビット演算 `bitand` `bitor` など |

`indexOf` は添字と同じくバイト位置を返すので、`s[indexOf(s, "x"):]` がそのまま使える。一方、`len` は文字数を返すので、ASCII以外を含む文字列では `len` の値を添字の計算に使えない。

### ユーザー定義関数

```rust
let schema = Schema::builder()
    .func("isHoliday", |d: Timestamp| -> bool { /* ... */ })
    .func_fallible("rate", |code: &str| -> Result<f64, MyError> { /* ... */ })
    .build();
```

引数と戻り値のRust型から `FuncSig` を作るので、型を手で書く必要はない。関数には副作用がないことを前提とする。同じ引数に必ず同じ値を返す関数には `pure` を指定でき、引数が定数ならOptimizerが畳み込む。

## ルールエンジンAPI

外部に見せるのは `Rule<T>` と `RuleSet<T>` の二つ。`T` はルールの戻り値型で、戻り値の型が `T` と合わないルールはコンパイルの時点でエラーになる。

```rust
use dictum::{Rule, RuleSet, Schema, Ctx};

// 単体のルール
let rule: Rule<bool> = Rule::compile(r#"user.Group in ["admin"] && user.Age >= 18"#, &schema)?;
let ok = rule.eval(&Ctx::new(&facts))?;

// 理由つき
let report = rule.explain(&Ctx::new(&facts))?;
println!("{report}");

// 複数のルール
let mut set = RuleSet::<bool>::new(schema.clone());
set.insert("adult", "user.Age >= 18")?;
set.insert("staff", r#"user.Group in ["admin", "moderator"]"#)?;
let matched: Vec<&str> = set.matching(&Ctx::new(&facts))?;
```

### Ctx

評価ごとの入力をまとめる。事実データ、時計（`now()` の値）、実行上限を持つ。時計も入力として外から渡すので、事実データと時計が同じなら同じ結果になる（ユーザー定義関数が `pure` である限り）。

### explain

評価したノードごとに、Span・値・子ノードを持つ木を返す。短絡評価で飛ばしたノードは `Skipped` になる。

```
user.Group in ["admin"] && user.Age >= 18   → false
├─ user.Group in ["admin"]                 → true   (user.Group = "admin")
└─ user.Age >= 18                          → false  (user.Age = 16)
```

通常の `eval` を遅くしないよう、記録つきのクロージャ木は `explain` が初めて呼ばれたときに別に生成する。

### ルールをまたいだ共通部分式の最適化

`RuleSet` に入った複数のルールに現れる同じ部分式（例：`user.Age >= 18`、`len(order.Items)`）は、一回の `matching` の中で1回だけ評価する。

1. 各ルールの型付きIRを正規化する（定数畳み込み後、`a == b` と `b == a` など可換な演算の引数を並べ替える）
2. 部分木ごとに構造ハッシュを取り、`RuleSet` 全体で2回以上現れる純粋な部分木にキャッシュ用のスロット番号を振る
3. その部分木のクロージャは「スロットに値があればそれを返し、なければ計算して保存する」形に包む
4. スロットは `Frame` が評価ごとに持ち、評価が終われば捨てる。`Rule` 自体は状態を持たないので `Send + Sync` のまま

対象は値が入力だけで決まる部分木で、`now()` と `pure` 指定のないユーザー定義関数を含むものは共有しない。実行上限のステップは実際に計算したときだけ数える。小さすぎる部分木（フィールド参照だけなど）はキャッシュの方が高くつくので、コストの下限を設ける。

### ホットリロード

- `Rule` と `RuleSet` は `Send + Sync` で、`Arc` で共有する。
- 更新時は新しい `RuleSet` をコンパイルし、`arc-swap` で差し替える。評価中のスレッドは古いルールのまま最後まで評価する。
- ルールはソース文字列で保存し、読み込み時にコンパイルする。コンパイル済み形式のシリアライズは提供しない。

## エラー報告

エラーは発生段階ごとに分け、すべてSpanとエラーコードを持つ。

```rust
pub enum Error {
    Compile(CompileError), // E01xx 構文、E02xx 型。複数件をまとめて持つ
    Eval(EvalError),       // E03xx: 実行時エラー。1件
}

pub struct EvalError { pub kind: EvalErrorKind, pub span: Span }

pub enum EvalErrorKind {
    Overflow, DivisionByZero, IndexOutOfBounds, CharBoundary,
    BudgetExceeded, TypeMismatch,   // Any 型の実行時チェック
    Function(Box<dyn core::error::Error + Send + Sync>),
}
```

`Display` はGo版と同じく該当箇所を `^` で示す。

```
error[E0201]: 型が合いません: int を期待しましたが string でした
 --> rule:1:12
  |
1 | user.Age + "x"
  |            ^^^ string
```

- エラー文は日英どちらにもできるよう、コードと引数を分けて持つ。
- エラー表示は外部クレートに依存しない。`miette` featureを有効にすると `miette::Diagnostic` を実装する。

### 複数エラーの扱い

コンパイル時のエラーはまとめて返し、実行時エラーは最初の1件で止める。rustcやTypeScriptと同じ考え方で、ルールを書く人が一度のコンパイルで複数の誤りをまとめて確認できるようにする。

| 段階 | 方針 |
| --- | --- |
| Parser | 同期点（`,` `)` `]` `}` `;`）まで読み飛ばして続行する。壊れた箇所は `ExprKind::Error` にする |
| Checker | 型エラーの箇所を `Type::Error` にして続行する。`Type::Error` を含む式からは新しいエラーを出さないので、連鎖エラーは出ない |
| 上限 | 1ルールあたり最大20件。構文エラーがあれば型チェックはしない |
| 実行時 | 最初のエラーで `Err` を返す |

```rust
pub struct CompileError { pub diagnostics: Vec<Diagnostic> }   // 1件以上、位置順

pub struct Diagnostic {
    pub code: ErrorCode,      // E0201 など
    pub span: Span,
    pub args: Vec<Arg>,       // メッセージの引数（翻訳用）
    pub help: Option<Help>,   // 「`?.` を使ってください」など
}
```

`RuleSet::insert_many` は全ルールをコンパイルしてから、ルール名ごとのエラーをまとめて返す。

## テスト戦略

互換性はGo版のテストを移植して測る。クロージャ木の正しさは差分テストで確かめ、参照インタプリタと同じ結果を返すかを比べる。

| 種類 | 内容 | ツール |
| --- | --- | --- |
| 適合テスト | [expr-lang/expr](https://github.com/expr-lang/expr)のテストケースを `tests/conformance/*.toml` に移植。意図的な差分は `deviation = "..."` で理由を書く | 自前のランナー |
| 差分テスト | ランダムに生成した型付き式を、木をたどる参照インタプリタとクロージャ木の両方で評価し、結果を比べる | `proptest` |
| ファジング | Lexer、Parser、Checkerがどんな入力でもpanicしないこと | `cargo-fuzz` |
| スナップショット | エラー表示とexplainの出力 | `insta` |
| ベンチマーク | 代表的なルールをGo版と同じ条件で比較 | `criterion` |

参照インタプリタは速度を気にせず仕様どおりに書き、`dictum-eval` の `reference` featureの中に置く。

## マイルストーン

各段階は前の段階に依存する。完了条件を満たしたら次に進む。

1. 構文（`dictum-syntax`）：Lexer、Prattパーサ、AST、エラー表示。完了条件は、適合テストの全式がパースでき、ファジングでpanicしないこと
2. 参照評価：型なしの木をたどるインタプリタと `Value`。MVPの組み込み関数を実装し、適合テストを通す
3. 型チェック（`dictum-check`）：`Type`、`Schema`、型付きIR。`dictum-derive` の最小版もここで作る
4. 高速評価（`dictum-eval`）：Optimizer、クロージャコンパイル、実行上限。差分テストを通し、Go版とのベンチマークを取る
5. ルールエンジン（`dictum`）：`Rule`、`RuleSet`、`explain`、時計の注入。crates.ioに0.1を公開する
6. 拡張：残りの組み込み関数、CLIとREPL、WASM対応

## 決定事項

| 論点 | 決定 | 反映先 |
| --- | --- | --- |
| pipe・三項演算子・`??` の優先順位 | Go版のパーサで確認し、`not` の順位と `??` の混在禁止を含めてすべてGo版に合わせる | 構文仕様 |
| 比較の連鎖 | `<` `>` `<=` `>=` は連鎖可。中央の式は1回だけ評価。`==` などは連鎖不可 | 構文仕様、Go版との差分 |
| 文字列の添字 | バイト単位。文字境界でなければエラー。`len` は文字数のまま | Go版との差分、型システム |
| `Decimal` 型 | `decimal` featureで提供。`Float` との暗黙の混在は禁止 | 型システム、評価エンジン |
| ルールをまたいだ共通部分式 | `RuleSet` で実施。評価ごとのキャッシュスロット方式 | ルールエンジンAPI |
| エラーの件数 | コンパイル時はまとめて（最大20件）、実行時は1件 | エラー報告 |
| MSRVと `no_std` | MSRV 1.85。全クレート `no_std` + `alloc` 対応 | アーキテクチャとクレート構成 |
