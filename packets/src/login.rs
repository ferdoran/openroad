use bevy::prelude::Message;

use sro_macro::*;
use sro_macro_derive::*;

#[derive(Message, Serialize, Deserialize, ByteSize, Clone)]
#[sro_packet]
pub struct LoginRequest {
    pub content_id: u8,
    pub username: String,
    pub password: String,
    pub shard_id: u16,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LoginResponse {
    pub result: u8,
    #[sro_packet(when = "result == 0x01")]
    pub login_info: Option<LoginInfo>,

    #[sro_packet(when = "result == 0x02")]
    pub login_error: Option<LoginError>,

    #[sro_packet(when = "result == 0x03")]
    pub custom: Option<CustomError>,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LoginInfo {
    pub agent_token: u32,
    pub agent_ip: String,
    pub agent_port: u16,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LoginError {
    /// `1`..=`0xF`; only `1` and `2` carry a sub-struct. Kept a raw `u8` so a
    /// code we have no name for can never fail deserialization — see
    /// [`LoginError::failure`] for the classified view.
    pub error_code: u8,
    #[sro_packet(when = "error_code == 0x01")]
    pub wrong_attempt: Option<WrongAttempt>,

    #[sro_packet(when = "error_code == 0x02")]
    pub account_blocked_err: Option<AccountBlockedError>,
}

/// The `0xA102` failure reasons, named from the login-UI pump's switch
/// (`corpus/client-dec/0086bfc0_FUN_0086bfc0.c:236-443`). The pump switches on
/// an internal id that is the wire `error_code` **+ 1** `[S]`, anchored by the
/// three arms we already modelled: arm 2 is the password error (wire `1`), arm
/// 3 the blocked/ban arm (wire `2`), arm 4 `UIO_MSG_ERROR_OVERLAP` — "already
/// connected" — (wire `3`). See `docs/net-login-gateway.md` for the full table
/// with the UI string per code.
///
/// This is a *view* over [`LoginError`], not the wire type: the wire stays a
/// raw code plus two optional sub-structs, so an unnamed code decodes fine and
/// lands in [`LoginFailure::Unknown`] instead of failing the packet — the same
/// reasoning as `describe_agent_auth_error` in `agent/mod.rs`.
#[derive(Clone, Debug, PartialEq)]
pub enum LoginFailure {
    /// `1` — wrong password. The counter is `[U]` on the wire (see
    /// [`WrongAttempt`]), hence the `Option`: it is `None` if the server sent
    /// the code without one.
    WrongPassword(Option<WrongAttempt>),
    /// `2` — account blocked; the sub-type says why.
    Blocked(AccountBlock),
    /// `3` — the account is still connected elsewhere.
    AlreadyConnected,
    /// `4`, `6`, `7`, `8`, `9` — five distinct server-connect failures that
    /// share one UI string. The client renders the raw id alongside it
    /// (`FUN_00861890(..., 0x43, id)`, `:415`), so the code is kept.
    ServerConnect(u8),
    /// `5` — the server is busy.
    ServerBusy,
    /// `0xA` — insufficient IP.
    InsufficientIp,
    /// `0xB` — billing failed.
    BillingFailed,
    /// `0xC` — billing-related refusal.
    BillingRelated,
    /// `0xD` — adult-only server.
    AdultOnlyServer,
    /// `0xE` — teen-over-only server.
    TeenOverOnlyServer,
    /// `0xF` — an adult account on a teen server.
    TeenServerAdult,
    /// Anything else. The important arm: a new or server-specific code must
    /// degrade, never fail.
    Unknown(u8),
}

/// The `block_type` of an `error_code == 2` failure (`:257-403`). Only `1`
/// carries a payload.
#[derive(Clone, Debug, PartialEq)]
pub enum AccountBlock {
    /// `1` — a timed ban; `BanInfo` carries the reason and the end time.
    Banned(Option<BanInfo>),
    /// `2` — `UIO_MSG_ERROR_ACCOUNT_CONNECT_IMPOSSIBILE`, no payload.
    ConnectImpossible,
    /// `3` — `UIO_MSG_ERROR_THERE_IS_NO_ACCOUNT_INFO`, no payload.
    NoAccountInfo,
    /// `4` — `UIO_MSG_ERROR_GRATIS_USER_BLOCKED`, no payload.
    GratisUserBlocked,
    /// Anything else.
    Unknown(u8),
}

impl LoginError {
    /// Classify the raw wire fields. Never fails.
    pub fn failure(&self) -> LoginFailure {
        match self.error_code {
            1 => LoginFailure::WrongPassword(self.wrong_attempt.clone()),
            2 => LoginFailure::Blocked(match &self.account_blocked_err {
                Some(blocked) => blocked.block(),
                // `error_code == 2` with no sub-struct: the block type is the
                // one thing we cannot guess, so it stays `Unknown(0)`.
                None => AccountBlock::Unknown(0),
            }),
            3 => LoginFailure::AlreadyConnected,
            code @ (4 | 6 | 7 | 8 | 9) => LoginFailure::ServerConnect(code),
            5 => LoginFailure::ServerBusy,
            0xA => LoginFailure::InsufficientIp,
            0xB => LoginFailure::BillingFailed,
            0xC => LoginFailure::BillingRelated,
            0xD => LoginFailure::AdultOnlyServer,
            0xE => LoginFailure::TeenOverOnlyServer,
            0xF => LoginFailure::TeenServerAdult,
            code => LoginFailure::Unknown(code),
        }
    }
}

impl AccountBlockedError {
    /// Classify the raw `block_type`. Never fails.
    pub fn block(&self) -> AccountBlock {
        match self.block_type {
            1 => AccountBlock::Banned(self.ban_info.clone()),
            2 => AccountBlock::ConnectImpossible,
            3 => AccountBlock::NoAccountInfo,
            4 => AccountBlock::GratisUserBlocked,
            other => AccountBlock::Unknown(other),
        }
    }
}

/// Human-readable name for a `0xA102` `error_code`, mirroring
/// `describe_agent_auth_error`. The original renders a `textuisystem.txt`
/// string per code (the keys are tabulated in `docs/net-login-gateway.md`);
/// this is the fallback prose for logs and for a client without that table.
pub fn describe_login_error(code: u8) -> &'static str {
    match code {
        1 => "wrong password",
        2 => "account blocked",
        3 => "already connected",
        4 | 6 | 7 | 8 | 9 => "could not connect to the server",
        5 => "server busy",
        0xA => "insufficient IP",
        0xB => "billing failed",
        0xC => "billing related",
        0xD => "adult-only server",
        0xE => "teen-over-only server",
        0xF => "adult account on a teen server",
        _ => "unknown error",
    }
}

/// Rendered as "Password entry has failed {cur} out of {max} times."
///
/// ⚠️ **`[U]` on the wire.** The binary only shows the *internal* form — one
/// `u32` split `lo16`/`hi16` into the two format arguments
/// (`FUN_0086bfc0:243-255`) — so whether the wire carries `2 × u32` or a single
/// packed `u32`, and whether max precedes cur, is unresolved. **What settles
/// it:** one captured `0xA102` with `result == 2, error_code == 1`; all five
/// captured `0xa102` lines are `result == 1`. Left unchanged deliberately
/// (#465) rather than guessed at.
#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct WrongAttempt {
    pub max_attempts: u32,
    pub cur_attempts: u32,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct AccountBlockedError {
    pub block_type: u8,
    #[sro_packet(when = "block_type == 0x01")]
    pub ban_info: Option<BanInfo>,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct CustomError {
    pub unk_u8_0: u8,
    pub unk_u8_1: u8,
    pub message: String,
    pub unk_u16: u16,
}

#[derive(Serialize, Deserialize, ByteSize, Clone, Debug, PartialEq)]
pub struct BanInfo {
    pub reason: String,
    pub end_year: u16,
    pub end_month: u16,
    pub end_day: u16,
    pub end_hour: u16,
    pub end_second: u16,
    pub end_microsecond: u16,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
#[sro_packet]
pub struct LoginCaptchaChallenge {
    pub image_flag: u8,
    pub image_remain: u16,
    pub image_compressed: u16,
    /// `[U]`. Named `image_uncompressed` until 2026-08-14, which asserted
    /// something false: the field is a **constant `0x32C8` (13000)** in all five
    /// `packet_dump/0x2322.log` samples, while every payload inflates to exactly
    /// 1600 bytes = `image_width * image_height / 8`. Its real meaning is
    /// unknown; nothing reads it (`docs/net-login-gateway.md`).
    pub unk_0x32c8: u16,
    pub image_width: u16,
    pub image_height: u16,
    #[sro_packet(list_type = "by-size-field", size_field = "image_compressed")]
    pub image_data: Vec<u8>,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LoginCaptchaConfirmRequest {
    pub code: String,
}

#[derive(Message, Serialize, Deserialize, ByteSize, Clone, Debug)]
pub struct LoginCaptchaConfirmResponse {
    pub result: u8,
    #[sro_packet(when = "result == 0x02")]
    pub wrong_attempt: Option<WrongAttempt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: u8) -> LoginError {
        LoginError {
            error_code: code,
            wrong_attempt: None,
            account_blocked_err: None,
        }
    }

    /// #465: the client modelled `1` and `2` and nothing else, so `3`..=`0xF`
    /// rendered nothing. All 15 codes from the UI pump's switch
    /// (`FUN_0086bfc0:236-443`) are now named, and anything else degrades to
    /// `Unknown` instead of being silently dropped.
    #[test]
    fn every_login_error_code_is_classified() {
        assert_eq!(error(3).failure(), LoginFailure::AlreadyConnected);
        for code in [4u8, 6, 7, 8, 9] {
            assert_eq!(error(code).failure(), LoginFailure::ServerConnect(code));
        }
        assert_eq!(error(5).failure(), LoginFailure::ServerBusy);
        assert_eq!(error(0xA).failure(), LoginFailure::InsufficientIp);
        assert_eq!(error(0xB).failure(), LoginFailure::BillingFailed);
        assert_eq!(error(0xC).failure(), LoginFailure::BillingRelated);
        assert_eq!(error(0xD).failure(), LoginFailure::AdultOnlyServer);
        assert_eq!(error(0xE).failure(), LoginFailure::TeenOverOnlyServer);
        assert_eq!(error(0xF).failure(), LoginFailure::TeenServerAdult);

        // Unnamed codes must not be lost.
        assert_eq!(error(0x10).failure(), LoginFailure::Unknown(0x10));
        assert_eq!(error(0).failure(), LoginFailure::Unknown(0));
        assert_eq!(describe_login_error(0x10), "unknown error");

        // Every named code has prose; only the unnamed ones say "unknown".
        for code in 1u8..=0xF {
            assert_ne!(
                describe_login_error(code),
                "unknown error",
                "code {code:#x}"
            );
        }
    }

    /// The payload-bearing arms keep their sub-struct, and the three
    /// no-payload block types (`:382-403`) are represented.
    #[test]
    fn block_types_and_payload_arms_survive_classification() {
        let attempt = WrongAttempt {
            max_attempts: 3,
            cur_attempts: 1,
        };
        let mut wrong = error(1);
        wrong.wrong_attempt = Some(attempt.clone());
        assert_eq!(
            wrong.failure(),
            LoginFailure::WrongPassword(Some(attempt.clone()))
        );
        // The server may omit the counter — still a wrong-password failure.
        assert_eq!(error(1).failure(), LoginFailure::WrongPassword(None));

        for (block_type, expected) in [
            (2u8, AccountBlock::ConnectImpossible),
            (3, AccountBlock::NoAccountInfo),
            (4, AccountBlock::GratisUserBlocked),
            (5, AccountBlock::Unknown(5)),
        ] {
            let mut blocked = error(2);
            blocked.account_blocked_err = Some(AccountBlockedError {
                block_type,
                ban_info: None,
            });
            assert_eq!(blocked.failure(), LoginFailure::Blocked(expected));
        }

        let ban = BanInfo {
            reason: "botting".to_string(),
            end_year: 2026,
            end_month: 8,
            end_day: 14,
            end_hour: 21,
            end_second: 0,
            end_microsecond: 0,
        };
        let mut blocked = error(2);
        blocked.account_blocked_err = Some(AccountBlockedError {
            block_type: 1,
            ban_info: Some(ban.clone()),
        });
        assert_eq!(
            blocked.failure(),
            LoginFailure::Blocked(AccountBlock::Banned(Some(ban)))
        );
    }
}
