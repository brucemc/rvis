use anyhow::{Error, Result};
use byte_slice_cast::*;
use derive_more::{Display, Error};
use gst::prelude::*;
use gst::ElementExt;
use rustfft::{num_complex::Complex, FftPlanner};
use std::sync::mpsc;

pub static FFT_SIZE: usize = 800;

#[derive(Debug, Display, Error)]
#[display(fmt = "Missing element {}", _0)]
struct MissingElement(#[error(not(source))] &'static str);

pub struct Pipeline {
    gstreamer_pipeline: gst::Pipeline,
}

impl Pipeline {
    pub fn new(
        file_name: &std::string::String,
        sender: mpsc::SyncSender<Vec<f64>>,
    ) -> Result<Pipeline, Error> {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let mut fft_buffer = vec![
            Complex {
                re: 0.0_f32,
                im: 0.0_f32
            };
            FFT_SIZE
        ];
        let mut pos: usize = 0;

        let pipeline = Pipeline {
            gstreamer_pipeline: gst::Pipeline::new(Option::None),
        };

        let filesrc = gst::ElementFactory::make("filesrc", Option::None)
            .map_err(|_| MissingElement("src"))?;
        let mpeg_audio_parse = gst::ElementFactory::make("mpegaudioparse", Option::None)
            .map_err(|_| MissingElement("mpegaudioparse"))?;
        let mpg_audio_dec = gst::ElementFactory::make("mpg123audiodec", Option::None)
            .map_err(|_| MissingElement("mpg123audiodec"))?;
        let tee =
            gst::ElementFactory::make("tee", Option::None).map_err(|_| MissingElement("tee"))?;

        let audio_queue = gst::ElementFactory::make("queue", Option::None)
            .map_err(|_| MissingElement("audio_queue"))?;
        let audio_convert = gst::ElementFactory::make("audioconvert", Option::None)
            .map_err(|_| MissingElement("audio_convert"))?;
        let audio_resample = gst::ElementFactory::make("audioresample", Option::None)
            .map_err(|_| MissingElement("audio_resample"))?;
        let audio_sink = gst::ElementFactory::make("autoaudiosink", Option::None)
            .map_err(|_| MissingElement("audio_sink"))?;

        let app_queue = gst::ElementFactory::make("queue", Option::None)
            .map_err(|_| MissingElement("app_queue"))?;
        let app_convert = gst::ElementFactory::make("audioconvert", Option::None)
            .map_err(|_| MissingElement("app_convert"))?;
        let app_resample = gst::ElementFactory::make("audioresample", Option::None)
            .map_err(|_| MissingElement("app_resample"))?;
        let app_sink = gst::ElementFactory::make("appsink", Option::None)
            .map_err(|_| MissingElement("app_sink"))?;

        filesrc.set_property("location", &file_name)?;

        // Appsink andle S16 mono at a convenient sample rate.
        let caps = gst::Caps::new_simple(
            "audio/x-raw",
            &[
                ("format", &gst_audio::AUDIO_FORMAT_S16.to_str()),
                ("rate", &11025i32),
//                ("rate", &200i32),
                ("channels", &1i32),
                ("layout", &"non-interleaved"),
            ],
        );

        app_sink.set_property("caps", &caps)?;


        let elements = &[
            &filesrc,
            &mpeg_audio_parse,
            &mpg_audio_dec,
            &tee,
            &audio_queue,
            &audio_convert,
            &audio_resample,
            &audio_sink,
            &app_queue,
            &app_convert,
            &app_resample,
            &app_sink,
        ];

        let decode_pipeline = &[&filesrc, &mpeg_audio_parse, &mpg_audio_dec, &tee];
        let audio_pipeline = &[&audio_queue, &audio_convert, &audio_resample, &audio_sink];
        let app_pipeline = &[&app_queue, &app_convert, &app_resample, &app_sink];
        pipeline.gstreamer_pipeline.add_many(elements)?;
        gst::Element::link_many(decode_pipeline)?;
        gst::Element::link_many(audio_pipeline)?;
        gst::Element::link_many(app_pipeline)?;

        let tee_audio_pad = tee.get_request_pad("src_%u").unwrap();
        let queue_audio_pad = audio_queue.get_static_pad("sink").unwrap();
        tee_audio_pad.link(&queue_audio_pad)?;

        let tee_app_pad = tee.get_request_pad("src_%u").unwrap();
        let queue_app_pad = app_queue.get_static_pad("sink").unwrap();
        tee_app_pad.link(&queue_app_pad)?;

        let appsink = app_sink
            .dynamic_cast::<gst_app::AppSink>()
            .expect("Sink element is expected to be an appsink!");

        // Getting data out of the appsink is done by setting callbacks on it.
        // The appsink calls those handlers, as soon as data is available.
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                // Add a handler to the "new-sample" signal.
                .new_sample(move |appsink| {
                    // Pull the sample in question out of the appsink's buffer.
                    let sample: gst::Sample =
                        appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;

                    let buffer = sample.get_buffer().ok_or_else(|| {
                        gst::gst_element_error!(
                            appsink,
                            gst::ResourceError::Failed,
                            ("Failed to get buffer from appsink")
                        );

                        gst::FlowError::Error
                    })?;

                    // At this point, buffer is only a reference to an existing memory region somewhere.
                    // When we want to access its content, we have to map it while requesting the required
                    // mode of access (read, read/write).
                    // This type of abstraction is necessary, because the buffer in question might not be
                    // on the machine's main memory itself, but rather in the GPU's memory.
                    // So mapping the buffer makes the underlying memory region accessible to us.
                    // See: https://gstreamer.freedesktop.org/documentation/plugin-development/advanced/allocation.html
                    let map = buffer.map_readable().map_err(|_| {
                        gst::gst_element_error!(
                            appsink,
                            gst::ResourceError::Failed,
                            ("Failed to map buffer readable")
                        );

                        gst::FlowError::Error
                    })?;

                    // We know what format the data in the memory region has, since we requested
                    // it by setting the appsink's caps. So what we do here is interpret the
                    // memory region we mapped as an array of signed 16 bit integers.

                    let samples = map.as_slice_of::<i16>().map_err(|_| {
                        gst::gst_element_error!(
                            appsink,
                            gst::ResourceError::Failed,
                            ("Failed to interpret buffer as S16 PCM")
                        );

                        gst::FlowError::Error
                    })?;

                    for sample in samples {
                        if pos >= FFT_SIZE {
                            fft.process(&mut fft_buffer);
                            pos = 0;
                            sender
                                .send(
                                    fft_buffer
                                        .iter()
                                        .skip(FFT_SIZE / 2)
                                        .map(|v| {
                                            let x = 1.0 +
                                                ((v.norm() as f64) / FFT_SIZE as f64).log10();
                                            if x < 0.0 {
                                                0.0
                                            } else {
                                                x / 5.0
                                            }
                                        })
                                        .collect::<Vec<_>>(),
                                )
                                .unwrap();
                        }
                        fft_buffer[pos] = Complex::new(*sample as f32, 0.0 as f32);
                        pos += 1;
                    }

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        Ok(pipeline)
    }

//    pub fn get_current_state(&self) -> gst::State {
//        self.gstreamer_pipeline.get_current_state()
//    }

    pub fn play(&self) -> Result<(), Error> {
        self.gstreamer_pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }

    pub fn pause(&self) -> Result<(), Error> {
        self.gstreamer_pipeline.set_state(gst::State::Paused)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), Error> {
        self.gstreamer_pipeline.set_state(gst::State::Null)?;
        Ok(())
    }
}
