# Structs, impls, and traits

Plix's object model is **Rust's model in Plix clothes**: data and behavior
are separate, there is **no data inheritance**, and shared abstractions are
expressed as **traits**. Composition over inheritance, statically checked,
zero runtime cost added to programs that don't use it.

```plix
struct Point { x: float, y: float, label: str = "point" }

impl Point {
    func new(x: float, y: float) -> Point {      // associated constructor
        return Point { x: x, y: y };
    }
    func dist(&self, other: Point) -> float {    // &self = read-only borrow
        auto dx = self.x - other.x;
        auto dy = self.y - other.y;
        return sqrt(dx * dx + dy * dy);
    }
    func scale(&mut self, k: float) {            // &mut self = mutable borrow
        self.x = self.x * k;
        self.y = self.y * k;
    }
}

trait Shape {
    func area(&self) -> float;                   // required method
    func kind(&self) -> str { return "shape"; }  // default method
}

impl Shape for Vec2 {                            // trait implementation
    func area(&self) -> float { return self.x * self.y; }
}
```

## struct — the data

```plix
struct Vec2 { x: float, y: float, name: str = "v" }
```

- Fields are typed; defaults are allowed (evaluated once, at declaration).
- Construct with a **struct literal**: `Vec2 { x: 1.0, y: 2.0 }`.
  Field shorthands work: in scope of `x`, `y`, write `Vec2 { x, y }`.
  Missing-but-defaulted fields fill in; anything missing or unknown is a
  compile error (`E0063`/`E0609`); wrong types are `E0625`.
- `p.x` reads, `p.x = v` writes (with the field's coercion: `int → float`
  widens, else strict).
- Instances are ordinary first-class values: arrays of them, map values,
  closures capture them, `==` compares them structurally, `say`/repr print
  `Point { x: 1.0, y: 2.0, label: "point" }`.
- Memory-wise an instance is an `auto`-managed heap value (refcounted like
  arrays); `own Point { ... }` makes it an owned value with the usual
  move/borrow rules.

## impl — the behavior

```plix
impl Point {
    func new(x: float, y: float) -> Point { ... }   // no self → associated fn
    func dist(&self, o: Point) -> float { ... }     // has &self → method
    func scale(&mut self, k: float) { ... }         // &mut self → mutating
}
```

- **Receivers mirror Rust**: `&self` borrows read-only, `&mut self` mutably.
  Bare `self` is sugar for `&self`. `self` is an ordinary parameter in the
  body (`self.x`); method calls `p.dist(q)` bind the receiver by reference —
  cheap, no copy.
- Calling a `&mut self` method on a `const` value is a compile error
  (`E0594`).
- **Associated functions** (no receiver) are called as `Point.new(3.0, 4.0)`.
  If a `new` exists, `Point(3.0, 4.0)` is sugar for it — in both backends.
- Methods are resolved **statically** (the checker knows the struct type);
  at runtime, `p.method` also works through dynamic dispatch: field lookup
  first, then inherent methods, then trait methods (ambiguous trait methods
  with the same name error explicitly, asking for clarity).
- A bound method is a first-class value: `auto f = p.dist; f(q);`.

## trait — the interface

```plix
trait Shape {
    func area(&self) -> float;          // `;` → required
    func kind(&self) -> str { ... }     // body → default implementation
}
```

- Every method declares a receiver (`&self` or `&mut self`).
- `impl Shape for Point { ... }` must provide every required method with an
  identical signature (`E0046`, `E0053`); extra methods not in the trait are
  `E0407`; implementing the same trait twice for the same struct is `E0119`.
- **Trait bounds**: annotate a parameter with a trait name to accept any
  implementor — checked statically:

  ```plix
  func total_area(shapes: array) -> float {
      auto sum: float = 0.0;
      for (s in shapes) { sum += s.area(); }   // dynamic
      return sum;
  }
  func describe(s: Shape) -> str {             // static bound
      return "${s.kind()} area=${s.area()}";
  }
  describe(point);        // ok
  describe(42);           // E0277: the trait `Shape` is not implemented for int
  ```

- **No data inheritance.** There are no base classes, no field merging, no
  virtual hierarchies. Traits carry *no data*. If you want reuse: compose
  structs (`inner: Point`) and delegate — or write a default trait method.

## Design notes

- Nothing here changes untyped code: values are the same runtime values;
  an instance is a heap object whose methods are plain functions plus a
  receiver binding. The interpreter and the compiled binary share the
  dispatch rules exactly (see `examples/oop.px`, which runs byte-identical
  output in both modes).
- Method lookup cost: one hash lookup in the struct descriptor (cached by
  the borrow of the receiver); bound-method creation allocates one small
  object.
