use std::path::PathBuf;
use anyhow::{Result, bail, Context as _};
use clap::Parser;
use gstreamer as gst;
use gst::prelude::*;

#[derive(Debug, Parser)]
struct Opt {
    video_path: PathBuf,
    embedded_data: String,
}

fn main() -> Result<()> {
    env_logger::init();
    gst::init()?;
    let opt = Opt::parse();
    let video_path = opt.video_path;
    let data = opt.embedded_data;

    // Parse the pipeline we want to probe from a static in-line string.
    let mut context = gst::ParseContext::new();
    let pipeline = match gst::parse::launch_full("filesrc name=src ! queue name=queueafterfilesrc ! parsebin name=parse  matroskamux name=mux ! queue name=queuebeforefilesink ! filesink name=sink sync=false", Some(&mut context), gst::ParseFlags::empty()) {
        Ok(pipeline) => pipeline,
        Err(err) => match err.kind::<gst::ParseError>() {
            Some(gst::ParseError::NoSuchElement) => {
                bail!("missing gst elements: {}", context.missing_elements().join(","));
            },
            _ => {
                return Err(err.into());
            },
        }
    };

    let pipeline = pipeline.downcast::<gst::Pipeline>().expect("no matter, we know it's a pipeline");
    {
        let parse = pipeline.by_name("parse").expect("no matter, we know it's there");
        let pipeline = pipeline.clone();
        parse.connect_pad_added(move |_parse, src_pad| {
            log::trace!("pad added: {:?}", src_pad);
            // all connect to mux
            let mux = pipeline.by_name("mux").expect("no matter, we know it's there");
            let pad_templates = mux.pad_template_list();
            let mut pad_templates = pad_templates.iter().filter(|pad_template| {
                let sink_caps = pad_template.caps();
                let Some(src_caps) = src_pad.current_caps() else {
                    log::warn!("no caps on pad {:?}", src_pad);
                    return false;
                };
                src_caps.can_intersect(&sink_caps)
            });
            let Some(pad_template) = pad_templates.next() else {
                log::warn!("no pad template for {:?}", src_pad);
                return;
            };
            let sink_pad = mux.request_pad(&pad_template, None, None).expect("no matter, we know it's there");

            // queue
            let queue = gst::ElementFactory::make("queue").build().expect("no matter, we know it's connectable");
            pipeline.add(&queue).expect("no matter, we know it's connectable");

            // src -> queue -> mux
            src_pad.link(&queue.static_pad("sink").expect("no matter, we know it's connectable")).expect("no matter, we know it's connectable");
            queue.static_pad("src").expect("no matter, we know it's connectable").link(&sink_pad).expect("no matter, we know it's connectable");

            queue.sync_state_with_parent().expect("no matter, we know it's syncable");
            log::trace!("linked {:?} to {:?}", src_pad.name(), sink_pad.name());
        });
    }

    let filesrc = pipeline.by_name("src").expect("no matter, we know it's there");
    let video_path_value: glib::Value = video_path.clone().into();
    filesrc.set_property("location", video_path_value);

    let mux = pipeline.by_name("mux").expect("no matter, we know it's there");
    let tag_setter = mux.dynamic_cast::<gst::TagSetter>().expect("as far as I know, matroskamux is a tag setter");
    let tag_data = data.as_str();
    let tag_data: &&str = &tag_data;
    tag_setter.add_tag::<gst::tags::Comment>(tag_data, gst::TagMergeMode::Replace);

    let dir = video_path.parent().context("no parent dir")?;
    let file_basename = video_path.file_stem().context("no file stem")?;
    let output_video_path = dir.join(file_basename).with_extension("with_data.mkv");

    let sink = pipeline.by_name("sink").expect("no matter, we know it's there");
    let output_video_path_value: glib::Value = output_video_path.into();
    sink.set_property("location", output_video_path_value);
   
    play_pipeline_to_eos(&pipeline)?;

    Ok(())
}

fn play_pipeline_to_eos(pipeline: &gst::Pipeline) -> Result<()> {
    let bus = pipeline.bus().unwrap();
    log::trace!("pipeline state: {:?}", pipeline.current_state());

    pipeline.set_state(gst::State::Playing)?;
    log::trace!("pipeline state: {:?}", pipeline.current_state());

    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => {
                log::trace!("End of stream");
                break;
            },
            MessageView::Error(err) => {
                log::error!("Error from {:?}: {:?}", msg.src().map(|s| s.path_string()), err.error());
                Err(anyhow::anyhow!("Error from {:?}: {:?}", msg.src().map(|s| s.path_string()), err.error()))?;
            }
            _ => log::trace!("msg: {:?}", msg),
        }
    }

    pipeline.set_state(gst::State::Null)?;

    Ok(())
}

