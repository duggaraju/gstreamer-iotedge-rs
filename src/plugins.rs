use gstreamer::glib;
use gstreamer::prelude::*;
use gstreamer::subclass::prelude::*;
use gstreamer::*;
use once_cell::sync::Lazy;

// Struct containing all the element data
pub struct InferencePluginImpl {
    srcpad: Pad,
    sinkpad: Pad,
}

static CAT: Lazy<gstreamer::DebugCategory> = Lazy::new(|| {
    DebugCategory::new(
        "inference",
        DebugColorFlags::empty(),
        Some("InferencePlugin Element"),
    )
});

impl InferencePluginImpl {
    // Called whenever a new buffer is passed to our sink pad. Here buffers should be processed and
    // whenever some output buffer is available have to push it out of the source pad.
    // Here we just pass through all buffers directly
    //
    // See the documentation of Buffer and BufferRef to see what can be done with
    // buffers.
    fn sink_chain(
        &self,
        pad: &Pad,
        buffer: Buffer,
    ) -> Result<gstreamer::FlowSuccess, gstreamer::FlowError> {
        log!(CAT, obj: pad, "Handling buffer {:?}", buffer);
        self.srcpad.push(buffer)
    }

    // Called whenever an event arrives on the sink pad. It has to be handled accordingly and in
    // most cases has to be either passed to Pad::event_default() on this pad for default handling,
    // or Pad::push_event() on all pads with the opposite direction for direct forwarding.
    // Here we just pass through all events directly to the source pad.
    //
    // See the documentation of Event and EventRef to see what can be done with
    // events, and especially the EventView type for inspecting events.
    fn sink_event(&self, pad: &Pad, event: Event) -> bool {
        log!(CAT, obj: pad, "Handling event {:?}", event);
        self.srcpad.push_event(event)
    }

    // Called whenever a query is sent to the sink pad. It has to be answered if the element can
    // handle it, potentially by forwarding the query first to the peer pads of the pads with the
    // opposite direction, or false has to be returned. Default handling can be achieved with
    // Pad::query_default() on this pad and forwarding with Pad::peer_query() on the pads with the
    // opposite direction.
    // Here we just forward all queries directly to the source pad's peers.
    //
    // See the documentation of Query and QueryRef to see what can be done with
    // queries, and especially the QueryView type for inspecting and modifying queries.
    fn sink_query(&self, pad: &Pad, query: &mut QueryRef) -> bool {
        log!(CAT, obj: pad, "Handling query {:?}", query);
        self.srcpad.peer_query(query)
    }

    // Called whenever an event arrives on the source pad. It has to be handled accordingly and in
    // most cases has to be either passed to Pad::event_default() on the same pad for default
    // handling, or Pad::push_event() on all pads with the opposite direction for direct
    // forwarding.
    // Here we just pass through all events directly to the sink pad.
    //
    // See the documentation of Event and EventRef to see what can be done with
    // events, and especially the EventView type for inspecting events.
    fn src_event(&self, pad: &Pad, event: Event) -> bool {
        log!(CAT, obj: pad, "Handling event {:?}", event);
        self.sinkpad.push_event(event)
    }

    // Called whenever a query is sent to the source pad. It has to be answered if the element can
    // handle it, potentially by forwarding the query first to the peer pads of the pads with the
    // opposite direction, or false has to be returned. Default handling can be achieved with
    // Pad::query_default() on this pad and forwarding with Pad::peer_query() on the pads with the
    // opposite direction.
    // Here we just forward all queries directly to the sink pad's peers.
    //
    // See the documentation of Query and QueryRef to see what can be done with
    // queries, and especially the QueryView type for inspecting and modifying queries.
    fn src_query(&self, pad: &Pad, query: &mut QueryRef) -> bool {
        log!(CAT, obj: pad, "Handling query {:?}", query);
        self.sinkpad.peer_query(query)
    }
}

// This trait registers our type with the GObject object system and
// provides the entry points for creating a new instance and setting
// up the class data
#[glib::object_subclass]
impl ObjectSubclass for InferencePluginImpl {
    const NAME: &'static str = "inferenceplugin";
    type Type = InferencePlugin;
    type ParentType = Element;

    // Called when a new instance is to be created. We need to return an instance
    // of our struct here and also get the class struct passed in case it's needed
    fn with_class(klass: &Self::Class) -> Self {
        // Create our two pads from the templates that were registered with
        // the class and set all the functions on them.
        //
        // Each function is wrapped in catch_panic_pad_function(), which will
        // - Catch panics from the pad functions and instead of aborting the process
        //   it will simply convert them into an error message and poison the element
        //   instance
        // - Extract our InferencePlugin struct from the object instance and pass it to us
        //
        // Details about what each function is good for is next to each function definition
        let templ = klass.pad_template("sink").unwrap();
        let sinkpad = Pad::builder_with_template(&templ, Some("sink"))
            .chain_function(|pad, parent, buffer| {
                InferencePluginImpl::catch_panic_pad_function(
                    parent,
                    || Err(FlowError::Error),
                    |inference| inference.sink_chain(pad, buffer),
                )
            })
            .event_function(|pad, parent, event| {
                InferencePluginImpl::catch_panic_pad_function(
                    parent,
                    || false,
                    |inference| inference.sink_event(pad, event),
                )
            })
            .query_function(|pad, parent, query| {
                InferencePluginImpl::catch_panic_pad_function(
                    parent,
                    || false,
                    |inference| inference.sink_query(pad, query),
                )
            })
            .build();

        let templ = klass.pad_template("src").unwrap();
        let srcpad = Pad::builder_with_template(&templ, Some("src"))
            .event_function(|pad, parent, event| {
                InferencePluginImpl::catch_panic_pad_function(
                    parent,
                    || false,
                    |inference| inference.src_event(pad, event),
                )
            })
            .query_function(|pad, parent, query| {
                InferencePluginImpl::catch_panic_pad_function(
                    parent,
                    || false,
                    |inference| inference.src_query(pad, query),
                )
            })
            .build();

        // Return an instance of our struct and also include our debug category here.
        // The debug category will be used later whenever we need to put something
        // into the debug logs
        Self { srcpad, sinkpad }
    }
}

// Implementation of glib::Object virtual methods
impl ObjectImpl for InferencePluginImpl {
    // Called right after construction of a new instance
    fn constructed(&self) {
        // Call the parent class' ::constructed() implementation first
        self.parent_constructed();

        // Here we actually add the pads we created in InferencePlugin::new() to the
        // element so that GStreamer is aware of their existence.
        self.obj().add_pad(&self.sinkpad).unwrap();
        self.obj().add_pad(&self.srcpad).unwrap();
    }
}

impl GstObjectImpl for InferencePluginImpl {}

// Implementation of Element virtual methods
impl ElementImpl for InferencePluginImpl {
    // Set the element specific metadata. This information is what
    // is visible from gst-inspect-1.0 and can also be programatically
    // retrieved from the Registry after initial registration
    // without having to load the plugin in memory.
    fn metadata() -> Option<&'static subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<subclass::ElementMetadata> = Lazy::new(|| {
            subclass::ElementMetadata::new(
                "Inference",
                "Generic",
                "Does nothing with the data",
                "Prakash Duggaraju <duggaraju@gmail.com>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    // Create and add pad templates for our sink and source pad. These
    // are later used for actually creating the pads and beforehand
    // already provide information to GStreamer about all possible
    // pads that could exist for this type.
    //
    // Actual instances can create pads based on those pad templates
    fn pad_templates() -> &'static [PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<PadTemplate>> = Lazy::new(|| {
            // Our element can accept any possible caps on both pads
            let caps = Caps::new_any();
            let src_pad_template =
                PadTemplate::new("src", PadDirection::Src, PadPresence::Always, &caps).unwrap();

            let sink_pad_template =
                PadTemplate::new("sink", PadDirection::Sink, PadPresence::Always, &caps).unwrap();

            vec![src_pad_template, sink_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }

    // Called whenever the state of the element should be changed. This allows for
    // starting up the element, allocating/deallocating resources or shutting down
    // the element again.
    fn change_state(
        &self,
        transition: StateChange,
    ) -> Result<StateChangeSuccess, StateChangeError> {
        trace!(CAT, imp: self, "Changing state {:?}", transition);

        // Call the parent class' implementation of ::change_state()
        self.parent_change_state(transition)
    }
}

glib::wrapper! {
    pub struct InferencePlugin(ObjectSubclass<InferencePluginImpl>) @extends Element, Object;
}

unsafe impl Send for InferencePlugin {}
unsafe impl Sync for InferencePlugin {}

// Registers the type for our element, and then registers in GStreamer under
// the name "rsInferencePlugin" for being able to instantiate it via e.g.
// ElementFactory::make().
pub fn register(plugin: &Plugin) -> Result<(), glib::BoolError> {
    Element::register(
        Some(plugin),
        "inference",
        Rank::None,
        InferencePlugin::static_type(),
    )
}
