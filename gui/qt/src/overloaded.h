// One lambda per alternative, as one visitor, for std::visit over the bindings' variants: a
// command, a report or a session's event. Every alternative must have its lambda, or the visit
// does not compile, so that a variant that grows one is not passed over in silence.
#pragma once

template <typename... F> struct Overloaded : F... {
    using F::operator()...;
};
