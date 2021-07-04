#[macro_use]
#[allow(unused_imports)]

extern crate glium;
use std::sync::mpsc;
//use std::time::Duration;
mod pipeline;
mod waterfall;
mod kaleidoscope;
mod texture_shader;
use clap::{Arg, App};
use glium::glutin::event::ModifiersState;

#[derive(Clone)]
enum Visualisation {
    KALEIDOSCOPE,
    WATERFALL,
}

impl Default for Visualisation {
    fn default() -> Self { Visualisation::KALEIDOSCOPE }
}

#[derive(Default)]
struct State {
    file_name : Option<String>,
    full_screen : bool,
    visualisation: Visualisation,
}

fn main() {
    let matches = App::new("rvis")
        .version("0.1.0")
        .author("Bruce McIntosh <bruce.e.mcintosh@gmail.com>")
        .about("Rust Audio Visualisation")
        .arg(Arg::with_name("file")
            .short("f")
            .long("file")
            .takes_value(true)
            .help("Audio file name"))
        .arg(Arg::with_name("fullscreen")
            .short("m")
            .long("full")
            .takes_value(false)
            .help("Run full screen"))
        .get_matches();

    let mut state : State = State::default();

    state.full_screen = matches.is_present("fullscreen");
    state.file_name =  matches.value_of("file").map(|f | f.to_string());

    match state.file_name {
        Some(_) => { run_visualisation(&state); },
        _ => { println!("No file"); }
    }
}

fn run_visualisation(state : &State) {
    gst::init().unwrap();

    use glium::{glutin, Surface};

    let event_loop = glutin::event_loop::EventLoop::new();

    let wb = if state.full_screen {
        let pm = event_loop.primary_monitor();
        glutin::window::WindowBuilder::new()
            .with_fullscreen(Some(glutin::window::Fullscreen::Borderless(pm)))
    }
    else {
        glutin::window::WindowBuilder::new()
    };

    let cb = glutin::ContextBuilder::new();
    let display = glium::Display::new(wb, cb, &event_loop).unwrap();

    let (mpsc_sender, mpsc_receiver) = mpsc::sync_channel(22000);
    let mut pipeline: Option<pipeline::Pipeline> = None;
    pipeline::Pipeline::new(
        state.file_name.as_ref().unwrap(),
        mpsc_sender.clone(),
    )
    .map_err(|err| {
        println!("Error: could not create pipeline. {}", err);
        pipeline = Option::None;
    })
    .and_then(|p| {
        p.play()
            .map_err(|err| {
                println!("Error: could not play. {}", err);
                pipeline = Option::None;
            })
            .and_then(|_| {
                pipeline = Option::Some(p);
                Ok(())
            })
    })
    .ok();

    let mut wf = waterfall::Shader::new(&display, 80, &pipeline::FFT_SIZE/2);
    let mut ks = kaleidoscope::Shader::new(&display);
    let mut ts = texture_shader::Shader::new(&display);

    let wf_texture = glium::texture::Texture2d::empty(&display, 1240, 1024).unwrap();

    let mut current_visualisation = state.visualisation.clone();
    let mut shift_state = false;

    event_loop.run(move |event, _, control_flow| {
        match event {
            glutin::event::Event::WindowEvent { event, .. } => match event {
                glutin::event::WindowEvent::CloseRequested => {
                    *control_flow = glutin::event_loop::ControlFlow::Exit;
                    return;
                },
                glutin::event::WindowEvent::ModifiersChanged (modifier_state) => {
                    shift_state = modifier_state.shift();
                },
                glutin::event::WindowEvent::KeyboardInput {
                    device_id: _,
                    input,
                    is_synthetic: _,
                } => {
                    match input.virtual_keycode {
                        Some(glutin::event::VirtualKeyCode::Q) => {
                            *control_flow = glutin::event_loop::ControlFlow::Exit;
                        },
                        Some(glutin::event::VirtualKeyCode::A) => {
                            match &pipeline {
                                Some(p) => {
                                    p.stop()
                                        .map_err(|err| {
                                            println!("Error: could not stop. {}", err);
                                        })
                                        .ok();
                                    pipeline = Option::None;
                                },
                                _ => {}
                            }
                        },
                        Some(glutin::event::VirtualKeyCode::S) => {
                            match &pipeline {
                                None => {
                                    pipeline::Pipeline::new(&r"resources/youve_got_speed.mp3".to_string(), mpsc_sender.clone())
                                        .map_err(|err| {
                                            println!("Error: could not create pipeline. {}", err);
                                            pipeline = Option::None;
                                        })
                                        .and_then(|p| {
                                            p.play()
                                                .map_err(|err| {
                                                    println!("Error: could not play. {}", err);
                                                    pipeline = Option::None;
                                                })
                                                .and_then(|_| {
                                                    pipeline = Option::Some(p);
                                                    Ok(())
                                                })
                                        })
                                        .ok();
                                },
                                Some(p) => {
                                    p.play()
                                        .map_err(|err| {
                                            println!("Error: {}", err);
                                            pipeline = Option::None;
                                        })
                                        .ok();
                                }
                            }
                        },
                        Some(glutin::event::VirtualKeyCode::D) => {
                            match &pipeline {
                                Some(p) => {
                                    p.pause()
                                        .map_err(|err| {
                                            println!("Error: {}", err);
                                            pipeline = Option::None;
                                        })
                                        .ok();
                                },
                                _ => {}
                            }
                        },
                        Some(glutin::event::VirtualKeyCode::K) => {
                            current_visualisation = Visualisation::KALEIDOSCOPE;
                        },
                        Some(glutin::event::VirtualKeyCode::W) => {
                            current_visualisation = Visualisation::WATERFALL;
                        },
                        Some(glutin::event::VirtualKeyCode::Key0) => {
                            if shift_state {
                                wf.set_option(0);
                            }
                            else {
                                wf.set_option(1);
                            }
                        },
                        Some(glutin::event::VirtualKeyCode::Key1) => {
                            if shift_state {
                                wf.set_scroll(0);
                            }
                            else {
                                wf.set_scroll(1);
                            }
                        },
                        _ => return,
                    }
                    return;
                },
                _ => return,
            },
            glutin::event::Event::NewEvents(cause) => match cause {
                glutin::event::StartCause::ResumeTimeReached { .. } => (),
                glutin::event::StartCause::Init => (),
                _ => return,
            },
            _ => return,
        }

        let next_frame_time =
            std::time::Instant::now() + std::time::Duration::from_nanos(16_666_667);
        *control_flow = glutin::event_loop::ControlFlow::WaitUntil(next_frame_time);

        if let Ok(fft_data) = mpsc_receiver.try_recv() {
            wf.set_fft_data(&fft_data);
//            println!("fft data");
        }


        let mut framebuffer = glium::framebuffer::SimpleFrameBuffer::new(&display, &wf_texture).unwrap();

        let mut target = display.draw();
        target.clear_color(0.0, 0.0, 1.0, 1.0);

        wf.render(&mut framebuffer);
        match current_visualisation {
            Visualisation::WATERFALL => ts.render(&mut target, &wf_texture),
            Visualisation::KALEIDOSCOPE => ks.render(&mut target, &wf_texture),
        }



        target.finish().unwrap();
    });
}
