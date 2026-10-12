# Code style

This is a description of a coding style that every contributor **must** follow.
Please read the whole document before you start pushing code.

## Generics

All bounds on named generic parameters should be written in `where`, even when there is only one bound.
Keep a trailing comma after the last bound.

```rust
// GOOD
pub fn new<N, P>(id: u64, name: N, path: P) -> Self
where
    N: Into<String>,
    P: Into<PathBuf>,
{ ... }

// BAD
pub fn new<N: Into<String>, P: Into<PathBuf>>(id: u64, name: N, path: P) -> Self { ... }
```

```rust
// GOOD
impl<T> Trait for Wrap<T>
where
    T: Trait,
{ ... }

// BAD
impl<T: Trait> Trait for Wrap<T> { ... }
```

Apply the same rule to type definitions:

```rust
// GOOD
pub struct Wrapper<T>
where
    T: Processor,
{
    inner: T,
}

// BAD
pub struct Wrapper<T: Processor> {
    inner: T,
}
```

Combine bounds with `+`; put associated-type constraints next to their trait:

```rust
// GOOD
fn collect_names<I, S>(names: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    names.into_iter().map(Into::into).collect()
}
```

## Use `Self` where possible

When referring to the type for which block is implemented, prefer using `Self`, rather than the name of the type:

```rust
impl ErrorKind {
    // GOOD
    fn print(&self) {
        match self {
            Self::Io => println!("Io"),
            Self::Network => println!("Network"),
            Self::Json => println!("Json"),
        }
    }

    // BAD
    fn print(&self) {
        match self {
            ErrorKind::Io => println!("Io"),
            ErrorKind::Network => println!("Network"),
            ErrorKind::Json => println!("Json"),
        }
    }
}
```

```rust
impl<'a> Request<'a> {
    // GOOD
    fn new<C>(client: &'a Client, request_id: C) -> Self
    where
        C: Into<String>,
    { ... }

    // BAD
    fn new<C>(client: &'a Client, request_id: C) -> Request<'a>
    where
        C: Into<String>,
    { ... }
}
```

**Rationale:** `Self` is generally shorter, and it is easier to copy-paste code or rename the type.

## Prefer method-call syntax

When a method is available on a value, prefer method-call syntax over fully qualified call syntax.

```rust
// GOOD
worker.reset().await?;

// BAD
Reset::reset(&worker).await?;
```

Use fully qualified syntax only when it is required to disambiguate between multiple trait implementations or inherent methods with the same name.

**Rationale:** method-call syntax is shorter, more idiomatic, easier to read, and keeps the receiver as the clear focus of the expression.

## Return expressions

Use the final expression as the return value. Reserve `return` for an early exit.

```rust
// GOOD
fn id(&self) -> ItemId {
    self.id
}

// BAD
fn id(&self) -> ItemId {
    return self.id;
}
```

When a wrapper only delegates and the result types match, return the call directly:

```rust
// GOOD
pub async fn save(&self, item: Item) -> Result<(), Error> {
    self.inner.save(item).await
}

// BAD
pub async fn save(&self, item: Item) -> Result<(), Error> {
    let result: Result<(), Error> = self.inner.save(item).await;
    result
}
```

Keep `Ok(operation()?)` when `?` is needed to convert the inner error to the function's error type.

## Readability over brevity

Functional programming constructs are allowed, but the code must remain readable.
Prefer explicit loops and conditionals over complex functional programming constructs when it improves readability,
especially for complex logic with multiple nested conditions.

```rust
// GOOD (more readable)
fn has_runnable_job(jobs: &[Job]) -> bool {
    for job in jobs {
        if let Some(schedule) = job.schedule() {
            if schedule.is_due() && job.is_enabled() {
                return true;
            }
        }
    }

    false
}

// BAD (difficult to read)
fn has_runnable_job(jobs: &[Job]) -> bool {
    jobs.iter().any(
        |job| matches!(job.schedule(), Some(schedule) if schedule.is_due() && job.is_enabled()),
    )
}
```

Short iterator chains are fine for straightforward transformations:

```rust
// GOOD
let ids: Vec<JobId> = jobs.iter().map(|job| job.id()).collect();
let has_errors: bool = results.iter().any(Result::is_err);
```

**Rationale:** Functional programming is powerful and often concise, but readability should not be sacrificed. Complex nested conditions, guard clauses, and intricate pattern matching within closures can make code difficult to understand and debug. Choose the approach that makes the intent clearest.

## Explicit local variable types

Prefer explicit type annotations on local bindings, even when the compiler can infer the type immediately.
The default style should be `let value: Type = ...`, not `let value = ...`.

```rust
// GOOD
let database: &Database = self.inner.database().await?;
let oauth: &AuthorizationClient = self.oauth().await?;
let output: AuthorizationOutput = oauth.complete(database, callback_url).await?;
let handle: AccountHandle = open_account(account).await?;
let result: Result<(), Error> = self.add_account_to_manager(database, external_id, account).await;

// BAD
let database = self.inner.database().await?;
let oauth = self.oauth().await?;
let output = oauth.complete(database, callback_url).await?;
let handle = open_account(account).await?;
let result = self.add_account_to_manager(database, external_id, account).await;
```

This also applies when the constructor or function name makes the type seem obvious:

```rust
// GOOD
let notification: NotificationManager = NotificationManager::new(4096);
let transport: Transport = Transport::builder(config.transport.clone()).build();
let worker: Worker = Worker::new(Arc::new(transport));

// DISCOURAGED
let notification = NotificationManager::new(4096);
let transport = Transport::builder(config.transport.clone()).build();
let worker = Worker::new(Arc::new(transport));
```

This is a strong default, not an absolute syntactic requirement. Type inference is acceptable when an annotation would make the code materially worse, for example:

- destructuring patterns where adding a type would require an artificial temporary;
- anonymous or impractically verbose closure/iterator types;
- very small bindings where the explicit type would add noise without improving readability.

When in doubt, write the type.

Do not repeat the same type in both an annotation and a turbofish:

```rust
// GOOD
let ids: Vec<JobId> = jobs.iter().map(|job| job.id()).collect();

// BAD
let ids: Vec<JobId> = jobs.iter().map(|job| job.id()).collect::<Vec<JobId>>();
```

**Rationale:** explicit local types make data flow visible without requiring the reader or reviewer to infer types from constructors, trait methods, generic return values, or distant context. They also make refactors and code review easier.

## Visual separation of logical blocks

Whitespace should reflect the logical structure of the code.
Do not add a blank line mechanically after every statement, but do not collapse distinct phases of a function into one uninterrupted block either.

Statements that belong to the same small operation may stay together. Add a blank line when the code moves to a different responsibility, for example from validation to I/O, from persistence to in-memory state, from construction to a side effect, or from the main operation to rollback/error handling.

```rust
// GOOD
if let Account::External(..) = &account {
    return Err(Error::ExternalAccountRequiresSetup);
}

if !self.inner.accounts_loaded() {
    return Err(Error::AccountsNotLoaded);
}

let database: &Database = self.inner.database().await?;

database.add_account(&external_id, &account).await?;

let res: Result<(), Error> = self
    .add_account_to_manager(database, external_id.clone(), account)
    .await;

match res {
    Ok(()) => Ok(()),
    Err(e) => {
        database.remove_account(&external_id).await?;

        Err(e)
    }
}

// BAD
if let Account::External(..) = &account {
    return Err(Error::ExternalAccountRequiresSetup);
}
if !self.inner.accounts_loaded() {
    return Err(Error::AccountsNotLoaded);
}
let database: &Database = self.inner.database().await?;
database.add_account(&external_id, &account).await?;
let res: Result<(), Error> = self
    .add_account_to_manager(database, external_id.clone(), account)
    .await;
match res {
    Ok(()) => Ok(()),
    Err(e) => {
        database.remove_account(&external_id).await?;
        Err(e)
    }
}
```

The important distinction is **semantic grouping**, not statement count. For example, several closely related local bindings can legitimately stay together:

```rust
let x: u64 = input.x();
let y: u64 = input.y();
let z: u64 = input.z();

let point: Point = Point::new(x, y, z);
```

There is no benefit in inserting a blank line between `x`, `y`, and `z`: they form one logical group. The blank line before `point` is useful because the code moves from extracting components to constructing a value.

Likewise, a multi-line method chain, constructor, `match`, or function call is one visual block and should not be split internally just to add whitespace.

Guard clauses should normally be visually separated from each other and from the main path when they represent independent preconditions.

Do not remove useful vertical spacing merely to make a function shorter on screen, and do not add spacing where no logical boundary exists.

**Rationale:** whitespace is part of the structure of the code. Good vertical spacing lets a reader identify the phases of a function before reading every expression in detail; arbitrary spacing only adds noise.

## Prefer named intermediate values

Prefer a named local variable when an intermediate value has semantic meaning, is reused, or makes the next operation easier to read.
Combine this rule with explicit local type annotations.

```rust
// GOOD
let transport: Transport = Transport::builder(config.transport.clone())
    .reconnect_delay_max(Duration::from_secs(15))
    .build();

let worker: Worker = Worker::new(Arc::new(transport));

// BAD
let worker: Worker = Worker::new(Arc::new(
    Transport::builder(config.transport.clone())
        .reconnect_delay_max(Duration::from_secs(15))
        .build(),
));
```

Do not introduce temporaries mechanically for every sub-expression. Introduce them when they create a meaningful step in the flow.

Likewise, avoid dense expressions that fetch, transform, validate, mutate state, and construct the final value all at once. Split those operations into readable stages.

**Rationale:** named intermediate values expose intent, provide natural places for type annotations and comments, and make debugging easier.

## Full paths for logging

Always write `tracing::<op>!(...)` instead of importing `use tracing::<op>;` and invoking `<op>!(...)`.

```rust
// GOOD
tracing::warn!("Everything is on fire");

// BAD
use tracing::warn;

warn!("Everything is on fire");
```

Include the operation and useful context, not just the error:

```rust
// GOOD
tracing::error!("Failed to save item {id}: {e}");

// BAD
tracing::error!("{e}");
```

**Rationale:**
- Less polluted import blocks
- Uniformity

## Order of imports

```rust
// First core, alloc and/or std
use core::fmt;
use std::{...};

// Second, external crates (including other workspace crates).
use crate_foo::{ ... };
use crate_bar::{ ... };

// If applicable, the current sub-modules
mod x;
mod y;

// Finally, the internal crate modules and submodules
use crate::{};
use super::{};
use self::y::Y;
```

## Import Style

Import structs, enums, traits, type aliases, and constants before using them. Do not write their
full module paths in signatures, type annotations, patterns, constructors, or variant paths.

```rust
// GOOD
use crate::types::UnixTimestamp;

fn started_at() -> UnixTimestamp {
    UnixTimestamp::now()
}

// BAD
fn started_at() -> crate::types::UnixTimestamp {
    crate::types::UnixTimestamp::now()
}
```

Function and macro calls may be module-qualified when the namespace makes the operation clearer,
as in `mem::take`.

```rust
// GOOD
use std::mem;

let previous: State = mem::take(&mut state);
```

Formatting items are the exception to the rule for types. Import `core::fmt` or `std::fmt`, then use
short paths such as `fmt::Display`, `fmt::Formatter`, and `fmt::Result`. Do not use full paths such
as `core::fmt::Result` or `std::fmt::Result` in code.

```rust
// GOOD
use core::fmt;

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { .. }
}

// BAD
impl core::fmt::Display for RenameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { .. }
}
```

When imports sub-modules:

```rust
// GOOD
mod x;

use self::x::Y;

// BAD
mod x;

use x::Y;
```

Group imports from the same module. Avoid wildcard imports in production code; existing preludes and `use super::*;` in tests are exceptions.

```rust
// GOOD
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};

// BAD
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::*;
```

In `fmt::Display`, write fixed text directly and use `write!` for formatted values. Do not allocate a temporary string:

```rust
// GOOD
match self {
    Self::Closed => f.write_str("closed"),
    Self::InvalidId(id) => write!(f, "invalid ID: {id}"),
}

// BAD
match self {
    Self::Closed => write!(f, "{}", "closed".to_string()),
    Self::InvalidId(id) => f.write_str(&format!("invalid ID: {id}")),
}
```

## If-let

Avoid the `if let ... { } else { }` construct if possible, use `match` instead:

```rust
// GOOD
match ctx.expected_type.as_ref() {
    Some(expected_type) => completion_ty == expected_type && !expected_type.is_unit(),
    None => false,
}

// BAD
if let Some(expected_type) = ctx.expected_type.as_ref() {
    completion_ty == expected_type && !expected_type.is_unit()
} else {
    false
}
```

Use `if let ... { }` when a match arm is intentionally empty:

```rust
// GOOD
if let Some(expected_type) = this.as_ref() {
    // Handle it
}

// BAD
match this.as_ref() {
    Some(expected_type) => {
        // Handle it
    },
    None => (),
}
```

Use `if let ... { return ...; }` for a single early-return case:

```rust
// GOOD
if let Account::External(..) = &account {
    return Err(Error::ExternalAccountRequiresSetup);
}

// BAD
match &account {
    Account::External(..) => {
        return Err(Error::ExternalAccountRequiresSetup);
    }
    _ => {}
}
```

Use `let ... else` when extracting a value and immediately returning or continuing if it is absent:

```rust
// GOOD
let Some(job) = jobs.get(&id) else {
    return Ok(());
};

job.run().await?;

// BAD
if jobs.contains_key(&id) {
    let job = jobs.get(&id).unwrap();
    job.run().await?;
} else {
    return Ok(());
}
```

## Sub-modules

Avoid the `mod x { .. }` construct if possible. Instead, create a file `x.rs` and define it with `mod x;`

**This applies to all sub-modules except `tests` and `benches`.**

```rust
// GOOD
mod x;

// BAD
mod x {
    ..
}
```

```rust
// GOOD
#[cfg(test)]
mod tests {
    ..
}

// BAD
mod tests;
```

```rust
// GOOD
#[cfg(bench)]
mod benches {
    ..
}

// BAD
mod benches;
```

## Type and implementation order

Keep each type close to its inherent implementation block, followed by its trait implementations.

Do not group all structs/enums first and then put all `impl` blocks at the bottom of the file.

```rust
// GOOD
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserId(u64);

pub struct User {
    id: UserId,
}

impl User {
    pub fn id(&self) -> UserId {
        self.id
    }
}

impl From<UserId> for User {
    fn from(id: UserId) -> Self {
        Self { id }
    }
}

pub enum UserKind {
    Admin,
    Regular,
}

impl UserKind {
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Admin)
    }
}

impl fmt::Display for UserKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admin => f.write_str("admin"),
            Self::Regular => f.write_str("regular"),
        }
    }
}
```

```rust
// BAD
pub struct User {
    id: UserId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserId(u64);

pub enum UserKind {
    Admin,
    Regular,
}

impl User {
    pub fn id(&self) -> UserId {
        self.id
    }
}

impl UserKind {
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Admin)
    }
}

impl From<UserId> for User {
    fn from(id: UserId) -> Self {
        Self { id }
    }
}

impl fmt::Display for UserKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admin => f.write_str("admin"),
            Self::Regular => f.write_str("regular"),
        }
    }
}
```

The expected order is:

```rust
struct/enum
impl Type
impl Trait for Type
```

Then repeat the same order for the next type.

**Rationale:** keeping the type definition, its constructors, methods, conversions, and trait implementations close together makes the file easier to read and avoids forcing readers to jump between the top and bottom of the file.

## Struct initialization

Use field-init shorthand when field and variable names match. Keep fields in declaration order.

```rust
pub struct Item {
    id: ItemId,
    name: String,
    enabled: bool,
}

// GOOD, inside impl Item
Self {
    id,
    name,
    enabled: true,
}

// BAD
Self {
    enabled: true,
    name: name,
    id: id,
}
```

Use `..Self::default()` for an intentional override of default fields, not to hide fields that the constructor must initialize explicitly.

## Error handling

Avoid `unwrap`, `expect`, and `panic!` in production code unless the invariant is truly impossible to violate or the failure is unrecoverable by design.

Prefer returning explicit errors.

```rust
// GOOD
let user_id: UserId = message.user_id.ok_or(Error::MissingUserId)?;

// BAD
let user_id = message.user_id.unwrap();
```

`expect` is acceptable in tests or when the message clearly documents the invariant:

```rust
// GOOD
let regex: Regex = Regex::new(PATTERN).expect("PATTERN must be a valid regular expression");

// BAD
let regex = Regex::new(PATTERN).unwrap();
```

Use `?` when an error only needs to be propagated:

```rust
// GOOD
let message: Message = parse_message(input)?;

// BAD
let message: Message = match parse_message(input) {
    Ok(message) => message,
    Err(e) => return Err(e),
};
```

Pass a conversion function directly when the closure adds nothing:

```rust
// GOOD
parse_record(input).map_err(Error::from)

// BAD
parse_record(input).map_err(|e| Error::from(e))
```

**Rationale:** explicit errors make failures easier to understand, test, and propagate. Panics should not be used as a substitute for proper error handling.

## Visibility

Use the most restrictive visibility possible.

```rust
// GOOD
pub(crate) struct ParserState {
    ...
}

// BAD
pub struct ParserState {
    ...
}
```

Prefer `pub(crate)`, `pub(super)`, or private items unless the type or function is intentionally part of the public API.

**Rationale:** a smaller public API is easier to maintain, refactor, document, and reason about.

## Function size and responsibility

Functions should have one clear responsibility.

Avoid large functions that mix parsing, validation, business logic, side effects, and formatting in the same block.

```rust
// GOOD
fn parse_message(input: &str) -> Result<Message, Error> {
    let header: Header = parse_header(input)?;

    let body: Body = parse_body(input)?;

    Message::new(header, body)
}

// BAD
fn parse_message(input: &str) -> Result<Message, Error> {
    // Parses headers
    // Validates fields
    // Reads external state
    // Builds the message
    // Logs several unrelated branches
    // Formats the output
    ...
}
```

If a function becomes hard to name, hard to test, or requires many comments to explain its internal flow, split it into smaller functions.

**Rationale:** small focused functions are easier to read, test, reuse, and review.

## Guard clauses

Prefer early returns when they reduce indentation and make invalid states explicit.

```rust
// GOOD
fn process_user(user: &User) -> Result<(), Error> {
    if !user.is_active() {
        return Err(Error::InactiveUser);
    }

    if user.is_banned() {
        return Err(Error::BannedUser);
    }

    process_active_user(user)
}

// BAD
fn process_user(user: &User) -> Result<(), Error> {
    if user.is_active() {
        if !user.is_banned() {
            process_active_user(user)
        } else {
            Err(Error::BannedUser)
        }
    } else {
        Err(Error::InactiveUser)
    }
}
```

**Rationale:** guard clauses keep the main path visible and reduce unnecessary nesting.

## Ownership and cloning

Do not use `.clone()` just to make the borrow checker happy.

Prefer borrowing when ownership is not required.

```rust
// GOOD
fn send_message(text: &str) {
    ...
}

send_message(&message.text);

// BAD
fn send_message(text: String) {
    ...
}

send_message(message.text.clone());
```

Clone only when ownership is actually needed, and the clone is intentional.

```rust
// GOOD
let cached_name: String = user.name.clone();
cache.insert(user.id, cached_name);
```

Take slices rather than references to containers when only a borrowed view is needed:

```rust
// GOOD
fn process(items: &[Item], name: &str) { ... }

// BAD
fn process(items: &Vec<Item>, name: &String) { ... }
```

Move a value when it is no longer needed instead of cloning it immediately before dropping the original:

```rust
// GOOD
cache.insert(id, item);

// BAD
cache.insert(id, item.clone());
drop(item);
```

**Rationale:** unnecessary cloning can hide ownership problems, increase allocations, and make data flow harder to follow.

## Public API design

Public APIs should be small, intentional, and hard to misuse.

Avoid exposing internal implementation details.

```rust
// GOOD
pub struct Client {
    inner: ClientInner,
}

impl Client {
    pub fn send(&self, message: Message) -> Result<(), Error> {
        self.inner.send(message)
    }
}

// BAD
pub struct Client {
    pub connection_pool: Pool,
    pub retry_state: RetryState,
    pub serializer: Serializer,
}
```

Prefer exposing behavior instead of internal fields.

**Rationale:** once an item is public, changing it becomes harder. Public APIs should describe what users can do, not how the implementation works internally.

## Match exhaustiveness

Prefer explicit `match` arms when each case has semantic meaning.

```rust
// GOOD
match status {
    Status::Pending => handle_pending(),
    Status::Running => handle_running(),
    Status::Done => handle_done(),
    Status::Failed(err) => handle_error(err),
}

// BAD
match status {
    Status::Done => handle_done(),
    _ => handle_other(),
}
```

Use `_` only when all ignored cases are intentionally handled in the same way.

**Rationale:** explicit matches make future changes safer. When a new enum variant is added, the compiler can help identify all places that need to be updated.

## Module boundaries

Modules should group related concepts and expose a clear boundary.

Avoid generic `utils`, `helpers`, or `common` modules when a more specific module name is possible.

```rust
// GOOD
mod parser;
mod validation;
mod transport;

// BAD
mod utils;
mod helpers;
mod common;
```

A module should have a clear purpose. If it contains unrelated functions, split it.

**Rationale:** specific modules are easier to navigate and make dependencies between parts of the codebase clearer.

## Module directory layout

The filesystem structure must reflect the module hierarchy.

When a module contains sub-modules, define the parent module using `mod.rs` inside its directory.

Do not use both `x.rs` and an adjacent `x/` directory.

```text
// GOOD
exchange/
├── mod.rs
├── history.rs
└── order.rs
```

```rust
// exchange/mod.rs
mod history;
mod order;
```

```text
// BAD
exchange.rs
exchange/
├── history.rs
└── order.rs
```

Conversely, when a module has no sub-modules, it must be defined as a standalone `.rs` file. Do not create a directory containing only `mod.rs`.

```text
// GOOD
src/
├── exchange.rs
├── parser.rs
└── transport.rs
```

```text
// BAD
src/
├── exchange/
│   └── mod.rs
├── parser/
│   └── mod.rs
└── transport/
    └── mod.rs
```

If `exchange` later needs its own sub-modules, move it from:

```text
src/
└── exchange.rs
```

to:

```text
src/
└── exchange/
    ├── mod.rs
    ├── history.rs
    └── order.rs
```

In short:

```text
Leaf module        -> x.rs
Module with children -> x/mod.rs + x/*.rs
```

**Rationale:** the filesystem should make the module hierarchy immediately obvious. Standalone modules should remain simple files, while directories should only be introduced when they are needed to contain sub-modules.
