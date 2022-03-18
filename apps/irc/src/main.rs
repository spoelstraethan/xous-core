#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

mod irc;
use irc::*;
mod repl;
use repl::*;
mod cmds;
use cmds::*;
use encoding::all::UTF_8;
use hiirc::*;
use num_traits::*;
use rkyv::*;
use std::{sync::Arc, thread};
use xous_ipc::Buffer;
use com::api::WlanStatus;

#[derive(Debug, num_derive::FromPrimitive, num_derive::ToPrimitive)]
pub(crate) enum ReplOp {
    /// a line of text has arrived
    Line = 0, // make sure we occupy opcodes with discriminants < 1000, as the rest are used for callbacks
    /// redraw our UI
    Redraw,
    /// change focus
    ChangeFocus,
    /// exit the application
    Quit,

    /// handle connection status changes
    NetStateUpdate,

    MessageReceived,
    MessageSent,
}

// This name should be (1) unique (2) under 64 characters long and (3) ideally descriptive.
pub(crate) const SERVER_NAME_REPL: &str = "_IRC demo application_";

#[xous::xous_main]
fn xmain() -> ! {
    log_server::init_wait().unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    log::info!("my PID is {}", xous::process::id());

    let xns = xous_names::XousNames::new().unwrap();
    // unlimited connections allowed, this is a user app and it's up to the app to decide its policy
    let sid = xns
        .register_name(SERVER_NAME_REPL, None)
        .expect("can't register server");
    // log::trace!("registered with NS -- {:?}", sid);

    let cb_cid = xous::connect(sid).unwrap();
    let mut netmgr = net::NetManager::new();
    netmgr.wifi_state_subscribe(cb_cid, ReplOp::NetStateUpdate.to_u32().unwrap()).unwrap();

    // this will make the IRC app appear to "hang" until wifi is connected by the system. this could be handled more gracefully, but
    // with a bit more code complexity.
    loop {
        let msg = xous::receive_message(sid).unwrap();
        match FromPrimitive::from_usize(msg.body.id()) {
            Some(ReplOp::NetStateUpdate) => {
                let buffer = unsafe {
                    xous_ipc::Buffer::from_memory_message(msg.body.memory_message().unwrap())
                };
                let wifi_status = WlanStatus::from_ipc(buffer.to_original::<com::WlanStatusIpc, _>().unwrap());
                if wifi_status.link_state == com_rs_ref::LinkState::Connected {
                    break;
                }
                // otherwise keep waiting
            }
            _ => {
                log::info!("Network not yet connected, IRC app cannot start");
            }
        }
    }

    let connection = IRCConnection {
        callback_sid: sid,
        nickname: "bunnie_precursor".to_string(),
        server: "irc.libera.chat:6667".to_string(),
        channel: DEFAULT_CHANNEL.to_string(),
        callback_new_message: ReplOp::MessageReceived.to_u32().unwrap(),
    };

    let new_message_sid = connection.connect();
    let new_message_cid =
        xous::connect(new_message_sid).expect("cannot connect to irc new message send");

    let mut repl = Repl::new(&xns, sid);
    let mut update_repl = true;
    let mut was_callback = false;
    let mut allow_redraw = false;
    loop {
        let msg = xous::receive_message(sid).unwrap();
        log::debug!("got message {:?}", msg);
        match FromPrimitive::from_usize(msg.body.id()) {
            Some(ReplOp::MessageReceived) => {
                let buffer =
                    unsafe { Buffer::from_memory_message(msg.body.memory_message().unwrap()) };
                let new_message = buffer
                    .to_original::<NewMessage, _>()
                    .expect("cannot unmarshal new received message");
                repl.circular_push(repl::History {
                    text: new_message.formatted(),
                    is_input: false,
                });

                update_repl = true; // set a flag, instead of calling here, so message can drop and calling server is released
                was_callback = false;
            }
            Some(ReplOp::MessageSent) => {}
            Some(ReplOp::Line) => {
                let buffer =
                    unsafe { Buffer::from_memory_message(msg.body.memory_message().unwrap()) };
                let s = buffer.as_flat::<xous_ipc::String<4000>, _>().unwrap();
                log::trace!("repl got input line: {}", s.as_str());

                let msg = s.as_str();
                //connection.send_message(&msg);

                {
                    let msg = NewMessage {
                        sender: None,
                        content: xous_ipc::String::from_str(msg),
                    };

                    let msgbuf = Buffer::into_buf(msg).expect("cannot mutate into buffer");
                    msgbuf
                        .send(new_message_cid, IRCOp::MessageSent.to_u32().unwrap())
                        .expect("cannot send new message to repl server");
                }

                repl.circular_push(repl::History {
                    text: msg.to_string(),
                    is_input: true,
                });

                update_repl = true; // set a flag, instead of calling here, so message can drop and calling server is released
                was_callback = false;
            }
            Some(ReplOp::Redraw) => {
                if allow_redraw {
                    repl.redraw().expect("REPL couldn't redraw");
                }
            }
            Some(ReplOp::ChangeFocus) => xous::msg_scalar_unpack!(msg, new_state_code, _, _, _, {
                let new_state = gam::FocusState::convert_focus_change(new_state_code);
                match new_state {
                    gam::FocusState::Background => {
                        allow_redraw = false;
                    }
                    gam::FocusState::Foreground => {
                        allow_redraw = true;
                    }
                }
            }),
            Some(ReplOp::Quit) => {
                log::error!("got Quit");
                break;
            }
            _ => {
                log::trace!("got unknown message, treating as callback");
                repl.msg(msg);
                update_repl = true;
                was_callback = true;
            }
        }
        if update_repl {
            repl.update(was_callback)
                .expect("REPL had problems updating");
            update_repl = false;
        }
        log::trace!("reached bottom of main loop");
    }
    // clean up our program
    log::error!("main loop exit, destroying servers");
    xns.unregister_server(sid).unwrap();
    xous::destroy_server(sid).unwrap();
    log::trace!("quitting");
    xous::terminate_process(0)
}
