//! A composable abstraction for building `Collector`s.
use tracing_core::{
    collect::{Collect, Interest},
    metadata::Metadata,
    span, Event, LevelFilter,
};

#[cfg(feature = "registry")]
use crate::registry::{self, LookupSpan, Registry, SpanRef};
use std::{any::TypeId, marker::PhantomData};

/// A composable handler for `tracing` events.
///
/// The [`Collector`] trait in `tracing-core` represents the _complete_ set of
/// functionality required to consume `tracing` instrumentation. This means that
/// a single `Collector` instance is a self-contained implementation of a
/// complete strategy for collecting traces; but it _also_ means that the
/// `Collector` trait cannot easily be composed with other `Collector`s.
///
/// In particular, [`Collector`]'s are responsible for generating [span IDs] and
/// assigning them to spans. Since these IDs must uniquely identify a span
/// within the context of the current trace, this means that there may only be
/// a single `Collector` for a given thread at any point in time &mdash;
/// otherwise, there would be no authoritative source of span IDs.
///
/// On the other hand, the majority of the [`Collector`] trait's functionality
/// is composable: any number of collectors may _observe_ events, span entry
/// and exit, and so on, provided that there is a single authoritative source of
/// span IDs. The `Subscriber` trait represents this composable subset of the
/// [`Collector`] behavior; it can _observe_ events and spans, but does not
/// assign IDs.
///
/// ## Composing Subscribers
///
/// Since a `Subscriber` does not implement a complete strategy for collecting
/// traces, it must be composed with a `Collector` in order to be used. The
/// `Subscriber` trait is generic over a type parameter (called `S` in the trait
/// definition), representing the types of `Collector` they can be composed
/// with. Thus, a `Subscriber` may be implemented that will only compose with a
/// particular `Collector` implementation, or additional trait bounds may be
/// added to constrain what types implementing `Collector` a `Subscriber` can wrap.
///
/// `Subscriber`s may be added to a `Collector` by using the [`SubscriberExt::with`]
/// method, which is provided by `tracing-subscriber`'s [prelude]. This method
/// returns a [`Layered`] struct that implements `Collector` by composing the
/// `Subscriber` with the `Collector`.
///
/// For example:
/// ```rust
/// use tracing_subscriber::Subscribe;
/// use tracing_subscriber::prelude::*;
/// use tracing::Collect;
///
/// pub struct MySubscriber {
///     // ...
/// }
///
/// impl<S: Collect> Subscribe<S> for MySubscriber {
///     // ...
/// }
///
/// pub struct MyCollector {
///     // ...
/// }
///
/// # use tracing_core::{span::{Id, Attributes, Record}, Metadata, Event};
/// impl Collect for MySubscriber {
///     // ...
/// #   fn new_span(&self, _: &Attributes) -> Id { Id::from_u64(1) }
/// #   fn record(&self, _: &Id, _: &Record) {}
/// #   fn event(&self, _: &Event) {}
/// #   fn record_follows_from(&self, _: &Id, _: &Id) {}
/// #   fn enabled(&self, _: &Metadata) -> bool { false }
/// #   fn enter(&self, _: &Id) {}
/// #   fn exit(&self, _: &Id) {}
/// }
/// # impl MySubscriber {
/// # fn new() -> Self { Self {} }
/// # }
/// # impl MyCollector {
/// # fn new() -> Self { Self { }}
/// # }
///
/// let collector = MySubscriber::new()
///     .with(MySubscriber::new());
///
/// tracing::collect::set_global_default(collector);
/// ```
///
/// Multiple `Subscriber`s may be composed in the same manner:
/// ```rust
/// # use tracing_subscriber::Subscribe;
/// # use tracing_subscriber::prelude::*;
/// # use tracing::Collect;
/// pub struct MyOtherSubscriber {
///     // ...
/// }
///
/// impl<S: Collect> Subscribe<S> for MyOtherSubscriber {
///     // ...
/// }
///
/// pub struct MyThirdSubscriber {
///     // ...
/// }
///
/// impl<S: Collect> Subscribe<S> for MyThirdSubscriber {
///     // ...
/// }
/// # pub struct MySubscriber {}
/// # impl<S: Collect> Subscribe<S> for MySubscriber {}
/// # pub struct MyCollector { }
/// # use tracing_core::{span::{Id, Attributes, Record}, Metadata, Event};
/// # impl Collect for MyCollector {
/// #   fn new_span(&self, _: &Attributes) -> Id { Id::from_u64(1) }
/// #   fn record(&self, _: &Id, _: &Record) {}
/// #   fn event(&self, _: &Event) {}
/// #   fn record_follows_from(&self, _: &Id, _: &Id) {}
/// #   fn enabled(&self, _: &Metadata) -> bool { false }
/// #   fn enter(&self, _: &Id) {}
/// #   fn exit(&self, _: &Id) {}
/// }
/// # impl MySubscriber {
/// # fn new() -> Self { Self {} }
/// # }
/// # impl MyOtherSubscriber {
/// # fn new() -> Self { Self {} }
/// # }
/// # impl MyThirdSubscriber {
/// # fn new() -> Self { Self {} }
/// # }
/// # impl MyCollector {
/// # fn new() -> Self { Self { }}
/// # }
///
/// let collector = MyCollector::new()
///     .with(MySubscriber::new())
///     .with(MyOtherSubscriber::new())
///     .with(MyThirdSubscriber::new());
///
/// tracing::collect::set_global_default(collector);
/// ```
///
/// The [`Subscribe::with_collector` method][with-col] constructs the `Layered`
/// type from a `Subscribe` and `Collect`, and is called by
/// [`SubscriberExt::with`]. In general, it is more idiomatic to use
/// `SubscriberExt::with`, and treat `Subscribe::with_collector` as an
/// implementation detail, as `with_collector` calls must be nested, leading to
/// less clear code for the reader. However, subscribers which wish to perform
/// additional behavior when composed with a subscriber may provide their own
/// implementations of `SubscriberExt::with`.
///
/// [`SubscriberExt::with`]: trait.SubscriberExt.html#method.with
/// [`Layered`]: struct.Layered.html
/// [prelude]: ../prelude/index.html
/// [with-col]: #method.with_collector
///
/// ## Recording Traces
///
/// The `Subscribe` trait defines a set of methods for consuming notifications from
/// tracing instrumentation, which are generally equivalent to the similarly
/// named methods on [`Collector`]. Unlike [`Collector`], the methods on
/// `Subscribe` are additionally passed a [`Context`] type, which exposes additional
/// information provided by the wrapped collector (such as [the current span])
/// to the subscriber.
///
/// ## Filtering with Subscribers
///
/// As well as strategies for handling trace events, the `Subscriber` trait may also
/// be used to represent composable _filters_. This allows the determination of
/// what spans and events should be recorded to be decoupled from _how_ they are
/// recorded: a filtering layer can be applied to other layers or
/// subscribers. A `Subscriber` that implements a filtering strategy should override the
/// [`register_callsite`] and/or [`enabled`] methods. It may also choose to implement
/// methods such as [`on_enter`], if it wishes to filter trace events based on
/// the current span context.
///
/// Note that the [`Subscribe::register_callsite`] and [`Subscribe::enabled`] methods
/// determine whether a span or event is enabled *globally*. Thus, they should
/// **not** be used to indicate whether an individual layer wishes to record a
/// particular span or event. Instead, if a subscriber is only interested in a subset
/// of trace data, but does *not* wish to disable other spans and events for the
/// rest of the subscriber stack should ignore those spans and events in its
/// notification methods.
///
/// The filtering methods on a stack of subscribers are evaluated in a top-down
/// order, starting with the outermost `Subscribe` and ending with the wrapped
/// [`Collector`]. If any subscriber returns `false` from its [`enabled`] method, or
/// [`Interest::never()`] from its [`register_callsite`] method, filter
/// evaluation will short-circuit and the span or event will be disabled.
///
/// [`Collector`]: https://docs.rs/tracing-core/latest/tracing_core/subscriber/trait.Subscriber.html
/// [span IDs]: https://docs.rs/tracing-core/latest/tracing_core/span/struct.Id.html
/// [`Context`]: struct.Context.html
/// [the current span]: struct.Context.html#method.current_span
/// [`register_callsite`]: #method.register_callsite
/// [`enabled`]: #method.enabled
/// [`on_enter`]: #method.on_enter
/// [`Subscribe::register_callsite`]: #method.register_callsite
/// [`Subscribe::enabled`]: #method.enabled
/// [`Interest::never()`]: https://docs.rs/tracing-core/latest/tracing_core/subscriber/struct.Interest.html#method.never
pub trait Subscribe<C>
where
    C: Collect,
    Self: 'static,
{
    /// Registers a new callsite with this subscriber, returning whether or not
    /// the subscriber is interested in being notified about the callsite, similarly
    /// to [`Collector::register_callsite`].
    ///
    /// By default, this returns [`Interest::always()`] if [`self.enabled`] returns
    /// true, or [`Interest::never()`] if it returns false.
    ///
    /// <div class="information">
    ///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
    /// </div>
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: This method (and <a href="#method.enabled">
    /// <code>Subscriber::enabled</code></a>) determine whether a span or event is
    /// globally enabled, <em>not</em> whether the individual subscriber will be
    /// notified about that span or event. This is intended to be used
    /// by subscribers that implement filtering for the entire stack. Subscribers which do
    /// not wish to be notified about certain spans or events but do not wish to
    /// globally disable them should ignore those spans or events in their
    /// <a href="#method.on_event"><code>on_event</code></a>,
    /// <a href="#method.on_enter"><code>on_enter</code></a>,
    /// <a href="#method.on_exit"><code>on_exit</code></a>, and other notification
    /// methods.
    /// </pre></div>
    ///
    /// See [the trait-level documentation] for more information on filtering
    /// with `Subscriber`s.
    ///
    /// Subscribers may also implement this method to perform any behaviour that
    /// should be run once per callsite. If the layer wishes to use
    /// `register_callsite` for per-callsite behaviour, but does not want to
    /// globally enable or disable those callsites, it should always return
    /// [`Interest::always()`].
    ///
    /// [`Interest`]: https://docs.rs/tracing-core/latest/tracing_core/struct.Interest.html
    /// [`Collector::register_callsite`]: https://docs.rs/tracing-core/latest/tracing_core/trait.Subscriber.html#method.register_callsite
    /// [`Interest::never()`]: https://docs.rs/tracing-core/latest/tracing_core/subscriber/struct.Interest.html#method.never
    /// [`Interest::always()`]: https://docs.rs/tracing-core/latest/tracing_core/subscriber/struct.Interest.html#method.always
    /// [`self.enabled`]: #method.enabled
    /// [`Subscriber::enabled`]: #method.enabled
    /// [`on_event`]: #method.on_event
    /// [`on_enter`]: #method.on_enter
    /// [`on_exit`]: #method.on_exit
    /// [the trait-level documentation]: #filtering-with-layers
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.enabled(metadata, Context::none()) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    /// Returns `true` if this subscriber is interested in a span or event with the
    /// given `metadata` in the current [`Context`], similarly to
    /// [`Collector::enabled`].
    ///
    /// By default, this always returns `true`, allowing the wrapped collector
    /// to choose to disable the span.
    ///
    /// <div class="information">
    ///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
    /// </div>
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: This method (and <a href="#method.register_callsite">
    /// <code>Subscriber::register_callsite</code></a>) determine whether a span or event is
    /// globally enabled, <em>not</em> whether the individual layer will be
    /// notified about that span or event. This is intended to be used
    /// by layers that implement filtering for the entire stack. Layers which do
    /// not wish to be notified about certain spans or events but do not wish to
    /// globally disable them should ignore those spans or events in their
    /// <a href="#method.on_event"><code>on_event</code></a>,
    /// <a href="#method.on_enter"><code>on_enter</code></a>,
    /// <a href="#method.on_exit"><code>on_exit</code></a>, and other notification
    /// methods.
    /// </pre></div>
    ///
    ///
    /// See [the trait-level documentation] for more information on filtering
    /// with `Subscriber`s.
    ///
    /// [`Interest`]: https://docs.rs/tracing-core/latest/tracing_core/struct.Interest.html
    /// [`Context`]: ../struct.Context.html
    /// [`Collector::enabled`]: https://docs.rs/tracing-core/latest/tracing_core/trait.Subscriber.html#method.enabled
    /// [`Subscriber::register_callsite`]: #method.register_callsite
    /// [`on_event`]: #method.on_event
    /// [`on_enter`]: #method.on_enter
    /// [`on_exit`]: #method.on_exit
    /// [the trait-level documentation]: #filtering-with-layers
    fn enabled(&self, metadata: &Metadata<'_>, ctx: Context<'_, C>) -> bool {
        let _ = (metadata, ctx);
        true
    }

    /// Notifies this layer that a new span was constructed with the given
    /// `Attributes` and `Id`.
    fn new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, C>) {
        let _ = (attrs, id, ctx);
    }

    // TODO(eliza): do we want this to be a public API? If we end up moving
    // filtering subscribers to a separate trait, we may no longer want `Subscriber`s to
    // be able to participate in max level hinting...
    #[doc(hidden)]
    fn max_level_hint(&self) -> Option<LevelFilter> {
        None
    }

    /// Notifies this subscriber that a span with the given `Id` recorded the given
    /// `values`.
    // Note: it's unclear to me why we'd need the current span in `record` (the
    // only thing the `Context` type currently provides), but passing it in anyway
    // seems like a good future-proofing measure as it may grow other methods later...
    fn on_record(&self, _span: &span::Id, _values: &span::Record<'_>, _ctx: Context<'_, C>) {}

    /// Notifies this subscriber that a span with the ID `span` recorded that it
    /// follows from the span with the ID `follows`.
    // Note: it's unclear to me why we'd need the current span in `record` (the
    // only thing the `Context` type currently provides), but passing it in anyway
    // seems like a good future-proofing measure as it may grow other methods later...
    fn on_follows_from(&self, _span: &span::Id, _follows: &span::Id, _ctx: Context<'_, C>) {}

    /// Notifies this subscriber that an event has occurred.
    fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, C>) {}

    /// Notifies this subscriber that a span with the given ID was entered.
    fn on_enter(&self, _id: &span::Id, _ctx: Context<'_, C>) {}

    /// Notifies this subscriber that the span with the given ID was exited.
    fn on_exit(&self, _id: &span::Id, _ctx: Context<'_, C>) {}

    /// Notifies this subscriber that the span with the given ID has been closed.
    fn on_close(&self, _id: span::Id, _ctx: Context<'_, C>) {}

    /// Notifies this subscriber that a span ID has been cloned, and that the
    /// subscriber returned a different ID.
    fn on_id_change(&self, _old: &span::Id, _new: &span::Id, _ctx: Context<'_, C>) {}

    /// Composes this subscriber around the given `Subscriber`, returning a `Layered`
    /// struct implementing `Subscriber`.
    ///
    /// The returned `Subscriber` will call the methods on this `Subscriber` and then
    /// those of the new `Subscriber`, before calling the methods on the subscriber
    /// it wraps. For example:
    ///
    /// ```rust
    /// # use tracing_subscriber::subscribe::Subscribe;
    /// # use tracing_core::Collect;
    /// pub struct FooSubscriber {
    ///     // ...
    /// }
    ///
    /// pub struct BarSubscriber {
    ///     // ...
    /// }
    ///
    /// pub struct MyCollector {
    ///     // ...
    /// }
    ///
    /// impl<S: Collect> Subscribe<S> for FooSubscriber {
    ///     // ...
    /// }
    ///
    /// impl<S: Collect> Subscribe<S> for BarSubscriber {
    ///     // ...
    /// }
    ///
    /// # impl FooSubscriber {
    /// # fn new() -> Self { Self {} }
    /// # }
    /// # impl BarSubscriber {
    /// # fn new() -> Self { Self { }}
    /// # }
    /// # impl MyCollector {
    /// # fn new() -> Self { Self { }}
    /// # }
    /// # use tracing_core::{span::{Id, Attributes, Record}, Metadata, Event};
    /// # impl tracing_core::Collect for MyCollector {
    /// #   fn new_span(&self, _: &Attributes) -> Id { Id::from_u64(1) }
    /// #   fn record(&self, _: &Id, _: &Record) {}
    /// #   fn event(&self, _: &Event) {}
    /// #   fn record_follows_from(&self, _: &Id, _: &Id) {}
    /// #   fn enabled(&self, _: &Metadata) -> bool { false }
    /// #   fn enter(&self, _: &Id) {}
    /// #   fn exit(&self, _: &Id) {}
    /// # }
    /// let collector = FooSubscriber::new()
    ///     .and_then(BarSubscriber::new())
    ///     .with_collector(MyCollector::new());
    /// ```
    ///
    /// Multiple subscribers may be composed in this manner:
    ///
    /// ```rust
    /// # use tracing_subscriber::subscribe::Subscribe;
    /// # use tracing_core::Collect;
    /// # pub struct FooSubscriber {}
    /// # pub struct BarSubscriber {}
    /// # pub struct MyCollector {}
    /// # impl<S: Collect> Subscribe<S> for FooSubscriber {}
    /// # impl<S: Collect> Subscribe<S> for BarSubscriber {}
    /// # impl FooSubscriber {
    /// # fn new() -> Self { Self {} }
    /// # }
    /// # impl BarSubscriber {
    /// # fn new() -> Self { Self { }}
    /// # }
    /// # impl MyCollector {
    /// # fn new() -> Self { Self { }}
    /// # }
    /// # use tracing_core::{span::{Id, Attributes, Record}, Metadata, Event};
    /// # impl tracing_core::Collect for MyCollector {
    /// #   fn new_span(&self, _: &Attributes) -> Id { Id::from_u64(1) }
    /// #   fn record(&self, _: &Id, _: &Record) {}
    /// #   fn event(&self, _: &Event) {}
    /// #   fn record_follows_from(&self, _: &Id, _: &Id) {}
    /// #   fn enabled(&self, _: &Metadata) -> bool { false }
    /// #   fn enter(&self, _: &Id) {}
    /// #   fn exit(&self, _: &Id) {}
    /// # }
    /// pub struct BazSubscriber {
    ///     // ...
    /// }
    ///
    /// impl<C: Collect> Subscribe<C> for BazSubscriber {
    ///     // ...
    /// }
    /// # impl BazSubscriber { fn new() -> Self { BazSubscriber {} } }
    ///
    /// let collector = FooSubscriber::new()
    ///     .and_then(BarSubscriber::new())
    ///     .and_then(BazSubscriber::new())
    ///     .with_collector(MyCollector::new());
    /// ```
    fn and_then<S>(self, subscriber: S) -> Layered<S, Self, C>
    where
        S: Subscribe<C>,
        Self: Sized,
    {
        Layered {
            subscriber,
            inner: self,
            _s: PhantomData,
        }
    }

    /// Composes this `Subscriber` with the given [`Collector`], returning a
    /// `Layered` struct that implements [`Collector`].
    ///
    /// The returned `Layered` subscriber will call the methods on this `Subscriber`
    /// and then those of the wrapped subscriber.
    ///
    /// For example:
    /// ```rust
    /// # use tracing_subscriber::subscribe::Subscribe;
    /// # use tracing_core::Collect;
    /// pub struct FooSubscriber {
    ///     // ...
    /// }
    ///
    /// pub struct MyCollector {
    ///     // ...
    /// }
    ///
    /// impl<C: Collect> Subscribe<C> for FooSubscriber {
    ///     // ...
    /// }
    ///
    /// # impl FooSubscriber {
    /// # fn new() -> Self { Self {} }
    /// # }
    /// # impl MyCollector {
    /// # fn new() -> Self { Self { }}
    /// # }
    /// # use tracing_core::{span::{Id, Attributes, Record}, Metadata};
    /// # impl tracing_core::Collect for MyCollector {
    /// #   fn new_span(&self, _: &Attributes) -> Id { Id::from_u64(0) }
    /// #   fn record(&self, _: &Id, _: &Record) {}
    /// #   fn event(&self, _: &tracing_core::Event) {}
    /// #   fn record_follows_from(&self, _: &Id, _: &Id) {}
    /// #   fn enabled(&self, _: &Metadata) -> bool { false }
    /// #   fn enter(&self, _: &Id) {}
    /// #   fn exit(&self, _: &Id) {}
    /// # }
    /// let collector = FooSubscriber::new()
    ///     .with_collector(MyCollector::new());
    ///```
    ///
    /// [`Collector`]: https://docs.rs/tracing-core/latest/tracing_core/trait.Collector.html
    fn with_collector(self, inner: C) -> Layered<Self, C>
    where
        Self: Sized,
    {
        Layered {
            subscriber: self,
            inner,
            _s: PhantomData,
        }
    }

    #[doc(hidden)]
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            Some(self as *const _ as *const ())
        } else {
            None
        }
    }
}

/// Extension trait adding a `with(Subscriber)` combinator to `Collector`s.
pub trait CollectorExt: Collect + crate::sealed::Sealed {
    /// Wraps `self` with the provided `layer`.
    fn with<S>(self, subscriber: S) -> Layered<S, Self>
    where
        S: Subscribe<Self>,
        Self: Sized,
    {
        subscriber.with_collector(self)
    }
}

/// Represents information about the current context provided to [`Subscriber`]s by the
/// wrapped [`Collector`].
///
/// To access [stored data] keyed by a span ID, implementors of the `Subscriber`
/// trait should ensure that the `Collector` type parameter is *also* bound by the
/// [`LookupSpan`]:
///
/// ```rust
/// use tracing::Collect;
/// use tracing_subscriber::{Subscribe, registry::LookupSpan};
///
/// pub struct MyCollector;
///
/// impl<C> Subscribe<C> for MyCollector
/// where
///     C: Collect + for<'a> LookupSpan<'a>,
/// {
///     // ...
/// }
/// ```
///
/// [`Subscriber`]: ../subscriber/trait.Subscriber.html
/// [`Collector`]: https://docs.rs/tracing-core/latest/tracing_core/trait.Subscriber.html
/// [stored data]: ../registry/struct.SpanRef.html
/// [`LookupSpan`]: "../registry/trait.LookupSpan.html
#[derive(Debug)]
pub struct Context<'a, S> {
    subscriber: Option<&'a S>,
}

/// A [`Collector`] composed of a `Collector` wrapped by one or more
/// [`Subscriber`]s.
///
/// [`Subscriber`]: ../subscriber/trait.Subscriber.html
/// [`Collector`]: https://docs.rs/tracing-core/latest/tracing_core/trait.Subscriber.html
#[derive(Clone, Debug)]
pub struct Layered<S, I, C = I> {
    subscriber: S,
    inner: I,
    _s: PhantomData<fn(C)>,
}

/// A Subscriber that does nothing.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    _p: (),
}

/// An iterator over the [stored data] for all the spans in the
/// current context, starting the root of the trace tree and ending with
/// the current span.
///
/// This is returned by [`Context::scope`].
///
/// [stored data]: ../registry/struct.SpanRef.html
/// [`Context::scope`]: struct.Context.html#method.scope
#[cfg(feature = "registry")]
#[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
pub struct Scope<'a, L: LookupSpan<'a>>(
    Option<std::iter::Chain<registry::FromRoot<'a, L>, std::iter::Once<SpanRef<'a, L>>>>,
);

// === impl Layered ===

impl<S, C> Collect for Layered<S, C>
where
    S: Subscribe<C>,
    C: Collect,
{
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        let outer = self.subscriber.register_callsite(metadata);
        if outer.is_never() {
            // if the outer subscriber has disabled the callsite, return now so that
            // the collector doesn't get its hopes up.
            return outer;
        }

        let inner = self.inner.register_callsite(metadata);
        if outer.is_sometimes() {
            // if this interest is "sometimes", return "sometimes" to ensure that
            // filters are reevaluated.
            outer
        } else {
            // otherwise, allow the inner subscriber or collector to weigh in.
            inner
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        if self.subscriber.enabled(metadata, self.ctx()) {
            // if the outer subscriber enables the callsite metadata, ask the collector.
            self.inner.enabled(metadata)
        } else {
            // otherwise, the callsite is disabled by the subscriber
            false
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        std::cmp::max(
            self.subscriber.max_level_hint(),
            self.inner.max_level_hint(),
        )
    }

    fn new_span(&self, span: &span::Attributes<'_>) -> span::Id {
        let id = self.inner.new_span(span);
        self.subscriber.new_span(span, &id, self.ctx());
        id
    }

    fn record(&self, span: &span::Id, values: &span::Record<'_>) {
        self.inner.record(span, values);
        self.subscriber.on_record(span, values, self.ctx());
    }

    fn record_follows_from(&self, span: &span::Id, follows: &span::Id) {
        self.inner.record_follows_from(span, follows);
        self.subscriber.on_follows_from(span, follows, self.ctx());
    }

    fn event(&self, event: &Event<'_>) {
        self.inner.event(event);
        self.subscriber.on_event(event, self.ctx());
    }

    fn enter(&self, span: &span::Id) {
        self.inner.enter(span);
        self.subscriber.on_enter(span, self.ctx());
    }

    fn exit(&self, span: &span::Id) {
        self.inner.exit(span);
        self.subscriber.on_exit(span, self.ctx());
    }

    fn clone_span(&self, old: &span::Id) -> span::Id {
        let new = self.inner.clone_span(old);
        if &new != old {
            self.subscriber.on_id_change(old, &new, self.ctx())
        };
        new
    }

    #[inline]
    fn drop_span(&self, id: span::Id) {
        self.try_close(id);
    }

    fn try_close(&self, id: span::Id) -> bool {
        #[cfg(feature = "registry")]
        let subscriber = &self.inner as &dyn Collect;
        #[cfg(feature = "registry")]
        let mut guard = subscriber
            .downcast_ref::<Registry>()
            .map(|registry| registry.start_close(id.clone()));
        if self.inner.try_close(id.clone()) {
            // If we have a registry's close guard, indicate that the span is
            // closing.
            #[cfg(feature = "registry")]
            {
                if let Some(g) = guard.as_mut() {
                    g.is_closing()
                };
            }

            self.subscriber.on_close(id, self.ctx());
            true
        } else {
            false
        }
    }

    #[inline]
    fn current_span(&self) -> span::Current {
        self.inner.current_span()
    }

    #[doc(hidden)]
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            return Some(self as *const _ as *const ());
        }
        self.subscriber
            .downcast_raw(id)
            .or_else(|| self.inner.downcast_raw(id))
    }
}

impl<C, A, B> Subscribe<C> for Layered<A, B, C>
where
    A: Subscribe<C>,
    B: Subscribe<C>,
    C: Collect,
{
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        let outer = self.subscriber.register_callsite(metadata);
        if outer.is_never() {
            // if the outer subscriber has disabled the callsite, return now so that
            // inner subscribers don't get their hopes up.
            return outer;
        }

        let inner = self.inner.register_callsite(metadata);
        if outer.is_sometimes() {
            // if this interest is "sometimes", return "sometimes" to ensure that
            // filters are reevaluated.
            outer
        } else {
            // otherwise, allow the inner subscriber or collector to weigh in.
            inner
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>, ctx: Context<'_, C>) -> bool {
        if self.subscriber.enabled(metadata, ctx.clone()) {
            // if the outer subscriber enables the callsite metadata, ask the inner subscriber.
            self.subscriber.enabled(metadata, ctx)
        } else {
            // otherwise, the callsite is disabled by this subscriber
            false
        }
    }

    #[inline]
    fn new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, C>) {
        self.inner.new_span(attrs, id, ctx.clone());
        self.subscriber.new_span(attrs, id, ctx);
    }

    #[inline]
    fn on_record(&self, span: &span::Id, values: &span::Record<'_>, ctx: Context<'_, C>) {
        self.inner.on_record(span, values, ctx.clone());
        self.subscriber.on_record(span, values, ctx);
    }

    #[inline]
    fn on_follows_from(&self, span: &span::Id, follows: &span::Id, ctx: Context<'_, C>) {
        self.inner.on_follows_from(span, follows, ctx.clone());
        self.subscriber.on_follows_from(span, follows, ctx);
    }

    #[inline]
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, C>) {
        self.inner.on_event(event, ctx.clone());
        self.subscriber.on_event(event, ctx);
    }

    #[inline]
    fn on_enter(&self, id: &span::Id, ctx: Context<'_, C>) {
        self.inner.on_enter(id, ctx.clone());
        self.subscriber.on_enter(id, ctx);
    }

    #[inline]
    fn on_exit(&self, id: &span::Id, ctx: Context<'_, C>) {
        self.inner.on_exit(id, ctx.clone());
        self.subscriber.on_exit(id, ctx);
    }

    #[inline]
    fn on_close(&self, id: span::Id, ctx: Context<'_, C>) {
        self.inner.on_close(id.clone(), ctx.clone());
        self.subscriber.on_close(id, ctx);
    }

    #[inline]
    fn on_id_change(&self, old: &span::Id, new: &span::Id, ctx: Context<'_, C>) {
        self.inner.on_id_change(old, new, ctx.clone());
        self.subscriber.on_id_change(old, new, ctx);
    }

    #[doc(hidden)]
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            return Some(self as *const _ as *const ());
        }
        self.subscriber
            .downcast_raw(id)
            .or_else(|| self.inner.downcast_raw(id))
    }
}

impl<S, C> Subscribe<C> for Option<S>
where
    S: Subscribe<C>,
    C: Collect,
{
    #[inline]
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        match self {
            Some(ref inner) => inner.register_callsite(metadata),
            None => Interest::always(),
        }
    }

    #[inline]
    fn enabled(&self, metadata: &Metadata<'_>, ctx: Context<'_, C>) -> bool {
        match self {
            Some(ref inner) => inner.enabled(metadata, ctx),
            None => true,
        }
    }

    #[inline]
    fn new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.new_span(attrs, id, ctx)
        }
    }

    #[inline]
    fn max_level_hint(&self) -> Option<LevelFilter> {
        match self {
            Some(ref inner) => inner.max_level_hint(),
            None => None,
        }
    }

    #[inline]
    fn on_record(&self, span: &span::Id, values: &span::Record<'_>, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.on_record(span, values, ctx);
        }
    }

    #[inline]
    fn on_follows_from(&self, span: &span::Id, follows: &span::Id, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.on_follows_from(span, follows, ctx);
        }
    }

    #[inline]
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.on_event(event, ctx);
        }
    }

    #[inline]
    fn on_enter(&self, id: &span::Id, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.on_enter(id, ctx);
        }
    }

    #[inline]
    fn on_exit(&self, id: &span::Id, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.on_exit(id, ctx);
        }
    }

    #[inline]
    fn on_close(&self, id: span::Id, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.on_close(id, ctx);
        }
    }

    #[inline]
    fn on_id_change(&self, old: &span::Id, new: &span::Id, ctx: Context<'_, C>) {
        if let Some(ref inner) = self {
            inner.on_id_change(old, new, ctx)
        }
    }

    #[doc(hidden)]
    #[inline]
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            Some(self as *const _ as *const ())
        } else {
            self.as_ref().and_then(|inner| inner.downcast_raw(id))
        }
    }
}

#[cfg(feature = "registry")]
#[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
impl<'a, S, C> LookupSpan<'a> for Layered<S, C>
where
    C: Collect + LookupSpan<'a>,
{
    type Data = C::Data;

    fn span_data(&'a self, id: &span::Id) -> Option<Self::Data> {
        self.inner.span_data(id)
    }
}

impl<S, C> Layered<S, C>
where
    C: Collect,
{
    fn ctx(&self) -> Context<'_, C> {
        Context {
            subscriber: Some(&self.inner),
        }
    }
}

// impl<L, S> Layered<L, S> {
//     // TODO(eliza): is there a compelling use-case for this being public?
//     pub(crate) fn into_inner(self) -> S {
//         self.inner
//     }
// }

// === impl CollectorExt ===

impl<C: Collect> crate::sealed::Sealed for C {}
impl<C: Collect> CollectorExt for C {}

// === impl Context ===

impl<'a, C> Context<'a, C>
where
    C: Collect,
{
    /// Returns the wrapped subscriber's view of the current span.
    #[inline]
    pub fn current_span(&self) -> span::Current {
        self.subscriber
            .map(Collect::current_span)
            // TODO: this would be more correct as "unknown", so perhaps
            // `tracing-core` should make `Current::unknown()` public?
            .unwrap_or_else(span::Current::none)
    }

    /// Returns whether the wrapped subscriber would enable the current span.
    #[inline]
    pub fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.subscriber
            .map(|subscriber| subscriber.enabled(metadata))
            // If this context is `None`, we are registering a callsite, so
            // return `true` so that the subscriber does not incorrectly assume that
            // the inner subscriber has disabled this metadata.
            // TODO(eliza): would it be more correct for this to return an `Option`?
            .unwrap_or(true)
    }

    /// Records the provided `event` with the wrapped collector.
    ///
    /// # Notes
    ///
    /// - The collector is free to expect that the event's callsite has been
    ///   [registered][register], and may panic or fail to observe the event if this is
    ///   not the case. The `tracing` crate's macros ensure that all events are
    ///   registered, but if the event is constructed through other means, the
    ///   user is responsible for ensuring that [`register_callsite`][register]
    ///   has been called prior to calling this method.
    /// - This does _not_ call [`enabled`] on the inner collector. If the
    ///   caller wishes to apply the wrapped collector's filter before choosing
    ///   whether to record the event, it may first call [`Context::enabled`] to
    ///   check whether the event would be enabled. This allows `Collectors`s to
    ///   elide constructing the event if it would not be recorded.
    ///
    /// [register]: https://docs.rs/tracing-core/latest/tracing_core/subscriber/trait.Subscriber.html#method.register_callsite
    /// [`enabled`]: https://docs.rs/tracing-core/latest/tracing_core/subscriber/trait.Subscriber.html#method.enabled
    /// [`Context::enabled`]: #method.enabled
    #[inline]
    pub fn event(&self, event: &Event<'_>) {
        if let Some(ref subscriber) = self.subscriber {
            subscriber.event(event);
        }
    }

    /// Returns metadata for the span with the given `id`, if it exists.
    ///
    /// If this returns `None`, then no span exists for that ID (either it has
    /// closed or the ID is invalid).
    #[inline]
    #[cfg(feature = "registry")]
    #[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
    pub fn metadata(&self, id: &span::Id) -> Option<&'static Metadata<'static>>
    where
        C: for<'lookup> LookupSpan<'lookup>,
    {
        let span = self.subscriber.as_ref()?.span(id)?;
        Some(span.metadata())
    }

    /// Returns [stored data] for the span with the given `id`, if it exists.
    ///
    /// If this returns `None`, then no span exists for that ID (either it has
    /// closed or the ID is invalid).
    ///
    /// <div class="information">
    ///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
    /// </div>
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: This requires the wrapped collector to implement the
    /// <a href="../registry/trait.LookupSpan.html"><code>LookupSpan</code></a> trait.
    /// See the documentation on <a href="./struct.Context.html"><code>Context</code>'s
    /// declaration</a> for details.
    /// </pre></div>
    ///
    /// [stored data]: ../registry/struct.SpanRef.html
    #[inline]
    #[cfg(feature = "registry")]
    #[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
    pub fn span(&self, id: &span::Id) -> Option<registry::SpanRef<'_, C>>
    where
        C: for<'lookup> LookupSpan<'lookup>,
    {
        self.subscriber.as_ref()?.span(id)
    }

    /// Returns `true` if an active span exists for the given `Id`.
    ///
    /// <div class="information">
    ///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
    /// </div>
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: This requires the wrapped subscriber to implement the
    /// <a href="../registry/trait.LookupSpan.html"><code>LookupSpan</code></a> trait.
    /// See the documentation on <a href="./struct.Context.html"><code>Context</code>'s
    /// declaration</a> for details.
    /// </pre></div>
    #[inline]
    #[cfg(feature = "registry")]
    #[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
    pub fn exists(&self, id: &span::Id) -> bool
    where
        C: for<'lookup> LookupSpan<'lookup>,
    {
        self.subscriber.as_ref().and_then(|s| s.span(id)).is_some()
    }

    /// Returns [stored data] for the span that the wrapped collector considers
    /// to be the current.
    ///
    /// If this returns `None`, then we are not currently within a span.
    ///
    /// <div class="information">
    ///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
    /// </div>
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: This requires the wrapped collector to implement the
    /// <a href="../registry/trait.LookupSpan.html"><code>LookupSpan</code></a> trait.
    /// See the documentation on <a href="./struct.Context.html"><code>Context</code>'s
    /// declaration</a> for details.
    /// </pre></div>
    ///
    /// [stored data]: ../registry/struct.SpanRef.html
    #[inline]
    #[cfg(feature = "registry")]
    #[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
    pub fn lookup_current(&self) -> Option<registry::SpanRef<'_, C>>
    where
        C: for<'lookup> LookupSpan<'lookup>,
    {
        let subscriber = self.subscriber.as_ref()?;
        let current = subscriber.current_span();
        let id = current.id()?;
        let span = subscriber.span(&id);
        debug_assert!(
            span.is_some(),
            "the subscriber should have data for the current span ({:?})!",
            id,
        );
        span
    }

    /// Returns an iterator over the [stored data] for all the spans in the
    /// current context, starting the root of the trace tree and ending with
    /// the current span.
    ///
    /// If this iterator is empty, then there are no spans in the current context.
    ///
    /// <div class="information">
    ///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
    /// </div>
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: This requires the wrapped subscriber to implement the
    /// <a href="../registry/trait.LookupSpan.html"><code>LookupSpan</code></a> trait.
    /// See the documentation on <a href="./struct.Context.html"><code>Context</code>'s
    /// declaration</a> for details.
    /// </pre></div>
    ///
    /// [stored data]: ../registry/struct.SpanRef.html
    #[cfg(feature = "registry")]
    #[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
    pub fn scope(&self) -> Scope<'_, C>
    where
        C: for<'lookup> registry::LookupSpan<'lookup>,
    {
        let scope = self.lookup_current().map(|span| {
            let parents = span.from_root();
            parents.chain(std::iter::once(span))
        });
        Scope(scope)
    }
}

impl<'a, C> Context<'a, C> {
    pub(crate) fn none() -> Self {
        Self { subscriber: None }
    }
}

impl<'a, C> Clone for Context<'a, C> {
    #[inline]
    fn clone(&self) -> Self {
        let subscriber = if let Some(ref subscriber) = self.subscriber {
            Some(*subscriber)
        } else {
            None
        };
        Context { subscriber }
    }
}

// === impl Identity ===
//
impl<C: Collect> Subscribe<C> for Identity {}

impl Identity {
    /// Returns a new `Identity` subscriber.
    pub fn new() -> Self {
        Self { _p: () }
    }
}

// === impl Scope ===

#[cfg(feature = "registry")]
#[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
impl<'a, L: LookupSpan<'a>> Iterator for Scope<'a, L> {
    type Item = SpanRef<'a, L>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.0.as_mut()?.next()
    }
}

#[cfg(feature = "registry")]
#[cfg_attr(docsrs, doc(cfg(feature = "registry")))]
impl<'a, L: LookupSpan<'a>> std::fmt::Debug for Scope<'a, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad("Scope { .. }")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct NopCollector;

    impl Collect for NopCollector {
        fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
            Interest::never()
        }

        fn enabled(&self, _: &Metadata<'_>) -> bool {
            false
        }

        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }

        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }

    struct NopSubscriber;
    impl<C: Collect> Subscribe<C> for NopSubscriber {}

    #[allow(dead_code)]
    struct NopSubscriber2;
    impl<C: Collect> Subscribe<C> for NopSubscriber2 {}

    /// A subscriber that holds a string.
    ///
    /// Used to test that pointers returned by downcasting are actually valid.
    struct StringSubscriber(String);
    impl<C: Collect> Subscribe<C> for StringSubscriber {}
    struct StringSubscriber2(String);
    impl<C: Collect> Subscribe<C> for StringSubscriber2 {}

    struct StringSubscriber3(String);
    impl<C: Collect> Subscribe<C> for StringSubscriber3 {}

    pub(crate) struct StringCollector(String);

    impl Collect for StringCollector {
        fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
            Interest::never()
        }

        fn enabled(&self, _: &Metadata<'_>) -> bool {
            false
        }

        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }

        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }

    fn assert_collector(_s: impl Collect) {}

    #[test]
    fn subscriber_is_collector() {
        let s = NopSubscriber.with_collector(NopCollector);
        assert_collector(s)
    }

    #[test]
    fn two_subscribers_are_collector() {
        let s = NopSubscriber
            .and_then(NopSubscriber)
            .with_collector(NopCollector);
        assert_collector(s)
    }

    #[test]
    fn three_subscribers_are_collector() {
        let s = NopSubscriber
            .and_then(NopSubscriber)
            .and_then(NopSubscriber)
            .with_collector(NopCollector);
        assert_collector(s)
    }

    #[test]
    fn downcasts_to_collector() {
        let s = NopSubscriber
            .and_then(NopSubscriber)
            .and_then(NopSubscriber)
            .with_collector(StringCollector("collector".into()));
        let collector =
            Collect::downcast_ref::<StringCollector>(&s).expect("collector should downcast");
        assert_eq!(&collector.0, "collector");
    }

    #[test]
    fn downcasts_to_subscriber() {
        let s = StringSubscriber("subscriber_1".into())
            .and_then(StringSubscriber2("subscriber_2".into()))
            .and_then(StringSubscriber3("subscriber_3".into()))
            .with_collector(NopCollector);
        let layer =
            Collect::downcast_ref::<StringSubscriber>(&s).expect("subscriber 2 should downcast");
        assert_eq!(&layer.0, "subscriber_1");
        let layer =
            Collect::downcast_ref::<StringSubscriber2>(&s).expect("subscriber 2 should downcast");
        assert_eq!(&layer.0, "subscriber_2");
        let layer =
            Collect::downcast_ref::<StringSubscriber3>(&s).expect("subscriber 3 should downcast");
        assert_eq!(&layer.0, "subscriber_3");
    }
}