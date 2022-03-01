use encoding::all::UTF_8;
use hiirc::*;
use num_traits::*;
use rkyv::*;
use std::{sync::Arc, thread};
use xous_ipc::Buffer;

pub(crate) const DEFAULT_CHANNEL: &str = "#precursor_irc_testing";
const MAX_MESSAGE_CHARS: usize = 512;
const MAX_NICKNAME_CHARS: usize = 8;

#[derive(Debug, num_derive::FromPrimitive, num_derive::ToPrimitive)]

pub(crate) enum IRCOp {
    /// When a message is sent by the user, send this message.
    MessageSent,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct NewMessage {
    pub content: xous_ipc::String<MAX_MESSAGE_CHARS>,
    pub sender: Option<xous_ipc::String<MAX_NICKNAME_CHARS>>,
}

impl NewMessage {
    pub fn formatted(&self) -> String {
        match self.sender {
            Some(sender) => format!("{} says:\n{}", sender, self.content),
            None => format!("{}", self.content),
        }
    }
}

struct ChannelListener {
    channel: String,

    /// Connection to UI
    main_cid: xous::CID,

    /// Callback ID to send over new message received over main_cid
    callback_new_channel_message_received: u32,

    /// SID on which listen for message to send to IRC channel
    send_message_sid: xous::SID,
}

impl ChannelListener {
    pub fn new(
        channel: String,
        main_cid: xous::CID,
        callback_new_channel_message_received: u32,
        send_message_sid: xous::SID,
    ) -> Self {
        ChannelListener {
            channel,
            main_cid,
            callback_new_channel_message_received,
            send_message_sid,
        }
    }
}

impl ChannelListener {
    fn message_loop(sid: xous::SID, irc_instance: Arc<Irc>, channel: &str) -> ! {
        loop {
            let msg = xous::receive_message(sid).unwrap();
            log::debug!("got message {:?}", msg);
            match FromPrimitive::from_usize(msg.body.id()) {
                Some(IRCOp::MessageSent) => {
                    log::trace!("sending message to channel");
                    let buffer =
                        unsafe { Buffer::from_memory_message(msg.body.memory_message().unwrap()) };
                    let new_message = buffer
                        .to_original::<NewMessage, _>()
                        .expect("cannot unmarshal new received message");

                    irc_instance.privmsg(channel, new_message.content.to_str());
                }
                _ => {}
            }
        }
    }

    fn start_listening_new_messages(&self, irc_instance: Arc<Irc>, channel: String) {
        let sid = self.send_message_sid.clone();
        let irc = irc_instance.clone();
        // start new xous messages server listener
        thread::spawn(move || {
            ChannelListener::message_loop(sid, irc, &channel);
        });
    }
}

// impl ChannelListener {
//     fn send_message(&self, msg: &String) {
//         if self.irc.is_none() {
//             return;
//         }

//         let i = self.irc.as_ref().unwrap();
//         i.privmsg(&self.channel, msg);
//     }
// }

impl Listener for ChannelListener {
    /// On any event we receive, print the Debug of it.
    fn any(&mut self, _: Arc<Irc>, event: &Event) {
        println!("{:?}", &event);
    }

    /// When the welcome message is received, join the channel.
    fn welcome(&mut self, irc: Arc<Irc>) {
        irc.join(&self.channel, None);
        //self.irc = Some(irc);
        self.start_listening_new_messages(irc, self.channel.clone());
    }

    fn channel_msg(
        &mut self,
        irc: Arc<Irc>,
        channel: Arc<Channel>,
        sender: Arc<ChannelUser>,
        message: &str,
    ) {
        log::trace!(
            "received new message from {}: {}",
            sender.nickname(),
            message
        );

        let msg = NewMessage {
            sender: Some(xous_ipc::String::from_str(&*sender.nickname())),
            content: xous_ipc::String::from_str(message),
        };

        let msgbuf = Buffer::into_buf(msg).expect("cannot mutate into buffer");
        msgbuf
            .send(self.main_cid, self.callback_new_channel_message_received)
            .expect("cannot send new message to repl server");
        //irc.privmsg(channel.name(), "hello!");
    }
}

pub(crate) struct IRCConnection {
    pub callback_sid: xous::SID,
    pub callback_new_message: u32,

    pub nickname: String,
    pub server: String,
    pub channel: String,
}

impl IRCConnection {
    pub fn connect(&self) -> xous::SID {
        let cid = xous::connect(self.callback_sid).expect("cannot connecto to main server");

        let receiving_msg_sid = xous::create_server().expect("cannot receiving message server");

        log::trace!("irc connecting...");

        thread::spawn({
            let channel = self.channel.clone();
            let nickname = self.nickname.clone();
            let server = self.server.clone();
            let cb_new_message = self.callback_new_message.clone();
            let receiving_msg_sid = receiving_msg_sid.clone();

            move || {
                let pk = ChannelListener::new(channel, cid, cb_new_message, receiving_msg_sid);

                Settings::new(&server, &nickname)
                    .dispatch(pk)
                    .expect("cannot connect to irc");
            }
        });

        receiving_msg_sid
    }
}
