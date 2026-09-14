#[allow(dead_code)]
pub struct SilkroadEncodingOptions {
    pub none: bool,
    pub encryption: bool,
    pub edc: bool,
    pub key_exchange: bool,
    pub key_challenge: bool,
}

impl From<u8> for SilkroadEncodingOptions {
    fn from(value: u8) -> Self {
        Self {
            none: value == 0,
            encryption: value & 2 != 0,
            edc: value & 4 != 0,
            key_exchange: value & 8 != 0,
            key_challenge: value & 16 != 0,
        }
    }
}

// pub struct SilkroadFrameEncoder {
//     pub(crate) security: Arc<RwLock<SilkroadSecurityState>>
// }
//
// pub struct SilkroadFrameDecoder {
//     pub(crate) security: Arc<RwLock<SilkroadSecurityState>>
// }
//
//
// impl Encoder<SilkroadFrame> for SilkroadFrameEncoder {
//     type Error = SilkroadFrameError;
//
//     fn encode(&mut self, item: SilkroadFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
//         let bytes = item.serialize(self.security.clone())?;
//         dst.extend_from_slice(&bytes);
//         Ok(())
//     }
// }
//
// impl Decoder for SilkroadFrameDecoder {
//     type Item = SilkroadFrame;
//     type Error = SilkroadFrameError;
//
//     fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
//         debug!("decoding frame: {:X}", src.clone().freeze());
//         match SilkroadFrame::parse(src, self.security.clone()) {
//             Ok((bytes_read, frame)) => {
//                 src.advance(bytes_read);
//                 Ok(Some(frame))
//             },
//             Err(SilkroadFrameError::Incomplete) => {
//                 error!("incomplete frame received");
//                 Ok(None)
//             },
//             Err(e) => Err(e)
//         }
//     }
// }
