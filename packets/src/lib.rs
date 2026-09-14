use std::io::Error;
use std::string::{FromUtf16Error, FromUtf8Error};

use bevy::log::trace;
use bevy::prelude::{Message, MessageReader, MessageWriter, PreUpdate};
use bytes::Bytes;
use paste::paste;
use thiserror::Error;

use sro_macro::error::SerializationError;

use crate::agent::prelude::*;
use crate::gateway::*;
use crate::global::*;
use crate::login::*;

pub mod agent;
pub mod gateway;
pub mod global;
pub mod login;

#[derive(Error, Debug)]
pub enum PacketError {
    #[error("The opcode '{0:#X}' was not recognized")]
    UnknownOpcode(u16),
    #[error("Could not (de)serialize a packet because of: {0}")]
    SerializationError(#[from] SerializationError),
}

impl From<Error> for PacketError {
    fn from(io_err: Error) -> Self {
        PacketError::SerializationError(SerializationError::from(io_err))
    }
}

impl From<FromUtf8Error> for PacketError {
    fn from(utf8_error: FromUtf8Error) -> Self {
        PacketError::SerializationError(SerializationError::from(utf8_error))
    }
}

impl From<FromUtf16Error> for PacketError {
    fn from(utf16_error: FromUtf16Error) -> Self {
        PacketError::SerializationError(SerializationError::from(utf16_error))
    }
}

macro_rules! packets {
    ($($opcode:literal => $name:ident),*) => {
        #[derive(Message)]
        pub enum Packet {
            $($name($name)),*
        }

        impl Packet {
            pub fn deserialize(opcode: u16, data: Bytes) -> Result<Packet, PacketError> {
                match opcode {
                    $($opcode => Ok(Packet::$name(data.try_into()?)),)*
                    _ => {
                        trace!("unparsable packet body: {:X}", data);
                        Err(PacketError::UnknownOpcode(opcode))
                    }
                }
            }

            pub fn into_serialize(self) -> (u16, Bytes) {
                match self {
                    $(Packet::$name(data) => ($opcode, data.into()),)*
                }
            }
        }

        $(
            impl From<$name> for Packet {
                fn from(other: $name) -> Self {
                    Packet::$name(other)
                }
            }

            paste! {
                #[allow(non_snake_case)]
                pub fn [< transform_net_event_ $name >](
                    mut r: MessageReader<Packet>,
                    mut w: MessageWriter<$name>
                ) {
                    for packet in r.read() {
                        match packet {
                            Packet::$name(data) => {w.write(data.clone());},
                            _ => {}
                        }
                    }
                }
            }
        )*

        pub trait NetworkExt {
            fn add_network_events(&mut self) -> &mut Self;
        }

        impl NetworkExt for bevy::app::App {
            fn add_network_events(&mut self) -> &mut Self {
                self.add_message::<Packet>();
                $(
                    self
                        .add_message::<$name>();

                    paste! {
                        self.add_systems(PreUpdate, [<transform_net_event_ $name>]);
                    }

                )*
                self
            }
        }

    }
}

// COUNTING RULE for this table — four planning docs once quoted four different
// "wired opcodes" numbers (85 / 67 / 171 / 176). They were not four measures: the
// two counting methods in use (this line-anchored grep and the macro-block parser
// in scripts/check_opcode_ledger.py) return the same set, so those were four
// undated snapshots of ONE number. State the date and the method, or the next
// reader re-derives the same confusion. One entry below = one wired opcode:
//
//   grep -cE '^\s*0x[0-9A-Fa-f]{4} => ' packets/src/lib.rs
//
// It counts BOTH directions. A direction split needs the ledger's Direction
// column (`docs/protocol/opcodes.md`), not this file: three opcodes (0x2001,
// 0x2113, 0x3080) carry traffic both ways, so the two directions add up to more
// than the total. Do not write the total into prose anywhere: since #638 the
// ledger deliberately carries no hand-maintained count and the gate rejects one
// coming back — `python3 scripts/check_opcode_ledger.py` prints it instead, and
// that command is the authority a doc should cite.
packets! {
    // First packet on every connection, both directions: a u16-length-prefixed
    // module name (docs/protocol/opcodes.md). The client announces "SR_Client"
    // (sro_client.exe 004ce8c0:16-27); the server answers with its own name,
    // which the original matches against GatewayServer/AgentServer/
    // DownloadServer (004ce9d0:62,91,138) -> ModuleIdentification::peer_kind().
    0x2001 => ModuleIdentification,
    0x2002 => KeepAlive,
    // XTrap anti-cheat challenge/response — one opcode, both directions, 1026
    // bytes each way: u8 kind, u8 sub, u8[0x400] (sro_client.exe 00842ff0:44-89,
    // docs/net-misc-0x2113.md). The original parses that framing inline in its
    // receive loop; the 1024-byte blob stays opaque, so the body is kept whole and
    // kind()/sub() expose the two header bytes. XTrapIdentification::stub_reply()
    // builds the one outbound shape we have. Nothing sends it — an auto-reply is
    // non-original behaviour and needs a config flag plus a live-session test.
    0x2113 => XTrapIdentification,
    0x2005 => GlobalStateUpdate,
    0x6005 => GlobalStateRequest,
    0x6102 => LoginRequest,
    0xA102 => LoginResponse,
    0x6323 => LoginCaptchaConfirmRequest,
    0xA323 => LoginCaptchaConfirmResponse,
    0x2322 => LoginCaptchaChallenge,

    0x6101 => ShardListRequest,
    0xA101 => ShardListResponse,
    0x6106 => ShardListPingRequest,
    0xA106 => ShardListPingResponse,


    0x6103 => AgentLoginRequest,
    0xA103 => AgentLoginResponse,
    0x7007 => CharacterSelectionActionRequest,
    0xB007 => CharacterSelectionActionResponse,
    0x7001 => CharacterJoinRequest,
    0xB001 => CharacterJoinResponse,

    // Post-join ingame packets (see agent/ingame.rs).
    0x34A5 => CharacterDataBegin,
    0x3013 => CharacterDataBody,
    0x34A6 => CharacterDataEnd,
    0x3020 => CelestialPosition,
    0x3027 => CelestialUpdate,
    // The real-world server clock (packed u32), pushed every ~10 min — a
    // different thing from the in-game time-of-day above.
    0x34BE => ServerTime,
    0x3012 => GameReady,

    // Group entity spawn/despawn (remote players, NPCs, monsters, item drops).
    // Note: 0x3018 is the (empty) End marker and 0x3019 carries the data — the
    // opposite of some server docs, confirmed against a live capture.
    0x3017 => GroupEntitySpawnBegin,
    0x3018 => GroupEntitySpawnEnd,
    0x3019 => GroupEntitySpawnData,
    0x3015 => SingleEntitySpawn,
    0x3016 => SingleEntityDespawn,

    // Movement.
    0x7021 => MovementRequest,
    0xB021 => MovementResponse,
    0xB023 => MovementPositionUpdate,
    // Turn-in-place. Only the S→C half is wired: the original parses 0xB024
    // (u32 uid + u16 angle) but has no builder for 0x7024 and an empty
    // dispatch stub for it, so the client→server body would be invented.
    // Wiring it needs a packet_dump/0x7024.log from a live idle turn.
    0xB024 => MovementAngleResponse,
    0x30D0 => EntitySpeedUpdate,
    0x30BF => EntityStateUpdate,
    0x3054 => EntityLevelUp,

    // Server notice push (unique spawned/killed). Subtype 5 is capture-verified
    // from packet_dump/0x300c.log; other subtypes keep their bytes.
    0x300C => NoticeUpdate,

    // Entity events (docs/net-entity-events-0x3011.md). Only the two with
    // evidence are wired: 0x3011 is source- and capture-verified, 0x304D rests
    // on a single capture. Their two siblings stay documented-only until a
    // capture exists — 0x305C ENTITY_DISPLAY_EFFECT (level-up glow, transform
    // and buff visuals) and 0x3091 EMOTE_USE (one opcode aliased in both
    // directions); neither has a parser in the original, so no layout is
    // invented for them here.
    0x3011 => CharacterDied,
    0x304D => DropUnlocked,

    // Death / resurrect: the death window's "Return" button (0x3053 CLIENT_GETUP,
    // empty body). C→S despite the 0x3xxx range — the documented exception; see
    // agent/ingame.rs and docs/re/notes/death-resurrect.md.
    0x3053 => GetUpRequest,

    // Hwan / berserk (mini-info jahwan button). The 0x70A7 action byte's enum
    // is [U] — see HwanActionRequest.
    0x70A7 => HwanActionRequest,
    0xB0A7 => HwanActionResponse,
    0x30DF => HwanLevelUpdate,

    // Vitals / points / stats (player mini-info HUD).
    0x3056 => ReceiveExperience,
    0x3057 => EntityBarsUpdate,
    0x304E => CharacterPointsUpdate,
    0x303D => CharacterStatsUpdate,

    // Misc captured world-join server pushes (EXPERIMENTAL — decoded from a live
    // vSRO 1.188 capture, see docs/net-captured-opcodes.md + PR #179; typed here
    // so the fan-out exists, client-side handling lands per-epic later).
    // 0x3153 own/gift/point silks are capture-VERIFIED; 0x3809 body is 2 bytes
    // capture-VERIFIED; 0x3305 and 0x3077 were captured empty, so their list
    // entry shapes are UNVERIFIED (see the struct docs).
    0x3153 => SilkUpdate,
    0x3809 => WeatherUpdate,
    0x3305 => FriendListInfo,
    0x3077 => CharacterFinished,
    // Fortress war (see agent/siege.rs, docs/net-siege-0x385F.md). 0x385F is a
    // u8 sub-command family; only the two arms our capture holds are decoded
    // (0x00 fortress list, 0x34 application-period end), the other 52 keep
    // their bytes. Five record fields have a verified width but no naming site,
    // so they stay unk_* until a wartime capture.
    0x385F => SiegeUpdate,
    // Not yet wired: 0x3206 SERVER_TICKET (server-unnamed, purpose unresolved —
    // skipped). See docs/net-captured-opcodes.md.

    // Invite / petition (docs/net-invite-0x3080.md). 0x3080 is dual-mapped:
    // S->C it is the petition popup, C->S the accept/decline, so its type is
    // the codec for both directions. The four funnel requests all carry a bare
    // target uid; the server raises the 0x3080 popup on that target.
    0x3080 => GameInvite,
    0x7062 => PartyInviteRequest,
    0x7081 => ExchangeInviteRequest,
    0x70F3 => GuildInviteRequest,
    0x7472 => AcademyInviteRequest,

    // Academy ("Training Camp") — the notice edit and the matching-board list
    // pair (agent/academy.rs, docs/net-academy-0x3C81.md). Layouts come from
    // the original's own builders/handlers (#261): xBot decodes none of this
    // family, which was mistaken for "no layout exists". 0x3C81 is NOT wired —
    // its handler reads zero bytes, so it lives in KNOWN_IGNORED_OPCODES.
    0x7477 => AcademyNoticeEditRequest,
    0x747D => AcademyMatchListRequest,
    0xB47D => AcademyMatchListResponse,
    0xB081 => ExchangeInviteResponse,
    // 0xB060 is the party-CREATE ack, wired with the party family below (#760):
    // the wiki pairs 0x7060/0xB060 and 0x7062/0xB062, and the invite ack this
    // comment was reaching for is 0xB062.
    // 0x70B3 is NOT a 0x3080 arm - it pairs with 0xB0B3 stall-talk-response -
    // and party-match 0x706D/0x306E has its own richer popup.

    // Entity selection (target window).
    0x7045 => SelectEntityRequest,
    0xB045 => SelectEntityResponse,

    // NPC talk (EXPERIMENTAL, per SilkroadDoc: 0x7046 = CLIENT_NPC_TALK,
    // 0x704B = CLIENT_NPC_CLOSE).
    0x7046 => TalkRequest,
    0xB046 => TalkResponse,
    0x704B => CloseTalkRequest,
    0xB04B => CloseTalkResponse,
    0x705A => TeleportRequest,
    0xB05A => TeleportResponse,
    // Cast/transition cancel — the cast gauge's own button. [V] from the
    // original's builder; vSRO never writes the 0xB05B reply, so expect silence
    // rather than an ack on a go-sro-derived server.
    0x705B => TransitionCastingCancelRequest,
    0xB05B => TransitionCastingCancelResponse,
    // Teleport completion handshake: the server resets the world and WAITS
    // for the client's complete ack before replaying CHARACTER_DATA.
    0x34B5 => GameReset,
    0x34B6 => GameResetComplete,
    // Item repair at an NPC (EXPERIMENTAL body — opcode names triple-sourced,
    // layout undocumented; see agent/inventory.rs).
    0x703E => ItemRepairRequest,
    0xB03E => ItemRepairResponse,

    // Object action: attacks (combat) + skill casting (underbar quickslots).
    0x7074 => ObjectActionRequest,
    0xB074 => ObjectActionResponse,
    0xB070 => ObjectActionUpdate,
    0xB071 => SkillEnd,

    // Skill/mastery learning — VERIFIED (#125, 2026-08-15). Requests read off
    // the original's builders (`sro_client.exe@0081dc60` writes one u32 for
    // 0x70A1; `@0081dd20` writes u32 + u8 for 0x70A2); acks capture-dated
    // 2026-08-05 in docs/net-skills-learn-buffs.md.
    0x70A1 => SkillLearnRequest,
    0xB0A1 => SkillLearnResponse,
    0x70A2 => MasteryLearnRequest,
    0xB0A2 => MasteryLearnResponse,

    // Skill/mastery level-DOWN — the mirror of the four above
    // (docs/net-mastery-teleport-0x7202.md). Both responses are read from the
    // original's parsers; neither request has a builder there, so their bodies are
    // mirrored from the level-UP siblings and stay unverified until a capture.
    0x7202 => SkillLevelDownRequest,
    0xB202 => MasterySkillLevelDownResponse,
    0x7203 => MasteryLevelDownRequest,
    0xB203 => MasteryLevelDownResponse,

    // Teleport recall point (0x7059 verified from the original's builder; the
    // 0xB059 ack has no parser in any source, so its body is kept whole and
    // log-only until packet_dump/0xb059.log exists).
    0x7059 => TeleportRecallRequest,
    0xB059 => TeleportRecallResponse,

    // Stat point spending (EXPERIMENTAL, per SilkroadDoc:
    // 0x7050 = CLIENT_INC_STR, 0x7051 = CLIENT_INC_INT).
    0x7050 => IncreaseStrRequest,
    0xB050 => IncreaseStrResponse,
    0x7051 => IncreaseIntRequest,
    0xB051 => IncreaseIntResponse,

    // Buffs (server → client). Both VERIFIED (#125): 0xB0BD capture-verified
    // 2026-08-06; 0xB072 is a count-prefixed LIST read off the original's
    // handler `sro_client.exe@008a4de0` (captures only ever showed one id and
    // could not separate that from a result byte).
    0xB0BD => BuffAdd,
    0xB072 => BuffRemove,

    // GM commands (e.g. /invisible).
    0x7010 => GmCommand,
    0xB010 => GmResponse,

    // Logout (Esc system window Quit/Restart).
    0x7005 => LogoutRequest,
    0xB005 => LogoutResponse,
    0x7006 => LogoutCancelRequest,
    0xB006 => LogoutCancelResponse,
    0x300A => LogoutSuccess,

    // Chat (see agent/chat.rs).
    0x7025 => ChatRequest,
    0xB025 => ChatResponse,
    0x3026 => ChatUpdate,
    0x302D => ChatRestriction,

    // Quest marks (see agent/quest.rs, docs/net-quest.md). The only two
    // opcodes of the quest family with evidence on this machine: both are
    // capture-verified from packet_dump/, and the family's other 15 are
    // listed unwired with their reasons in that doc (#758).
    0x30D6 => QuestMarkAdd,
    0x30D7 => QuestMarkRemove,

    // Party wire family (see agent/party.rs, docs/net-party-0x3065.md).
    // Spec-derived: no packet_dump exists for any of these yet.
    0x3065 => PartyData,
    0x3864 => PartyUpdate,
    0x306E => PartyMatchJoinResponse,
    0x7060 => PartyCreationRequest,
    0x7061 => PartyLeave,
    0x7063 => PartyKickRequest,
    0x7069 => PartyMatchCreationRequest,
    0x706A => PartyMatchEditedRequest,
    0x706B => PartyMatchDeleteRequest,
    0x706C => PartyMatchListRequest,
    0x706D => PartyMatchJoinNotify,
    0xB069 => PartyMatchCreationResponse,
    0xB06A => PartyMatchEditedResponse,
    0xB06B => PartyMatchDeleteResponse,
    0xB06C => PartyMatchListResponse,
    // The family remainder (#760): four of the five the seed never carried.
    // 0xB067 stays unwired — its body is recorded nowhere (docs/net-party.md).
    0x3068 => PartyDistribution,
    0xB060 => PartyCreateResponse,
    0xB062 => PartyInviteResponse,
    0xB06D => PartyMatchJoinAck,

    // Player exchange / trade (see agent/exchange.rs,
    // docs/net-exchange-0x3085.md). Spec-derived: no packet_dump exists yet.
    // The own-side staging half rides 0x7034/0xB034 sub-ops 4/5/13, still
    // missing from InventoryOperationRequest/Response.
    0x3085 => ExchangeStarted,
    0x3086 => ExchangePlayerConfirmed,
    0x3087 => ExchangeCompleted,
    0x3088 => ExchangeCanceled,
    0x3089 => ExchangeGoldUpdate,
    0x308C => ExchangeItemsUpdate,
    0x7082 => ExchangeConfirmRequest,
    0x7083 => ExchangeApproveRequest,
    0x7084 => ExchangeExitRequest,
    0xB082 => ExchangeConfirmResponse,
    0xB083 => ExchangeApproveResponse,
    0xB084 => ExchangeExitResponse,

    // Guild data / log / notice (see agent/guild.rs, docs/net-guild-0x3101.md).
    // Spec-derived: no packet_dump exists yet. 0x30FF and 0x38F5's per-type
    // payload stay raw because the original has no parser for them.
    0x34B3 => GuildDataBegin,
    0x3101 => GuildDataBody,
    0x34B4 => GuildDataEnd,
    0x30FF => GuildPlayerLog,
    0x38F5 => GuildUpdate,
    0xB0F0 => GuildCreatedData,
    0x70F9 => GuildNoticeEditRequest,

    // Guild lifecycle & membership acks (see agent/guild.rs,
    // docs/net-guild-lifecycle.md). One shared body form for the whole
    // 00881820-00881e70 handler cluster; 0xB0F6 is the only one with a
    // success payload. Still not wired here, deliberately: the union acks
    // 0xB0FB/0xB0FC/0xB0FD (guild union, #809) and the leadership acks
    // 0xB103/0xB105/0xB106/0xB107 (#811) — same cluster, different tickets.
    0x3100 => EntityGuildRemove,
    0xB0F1 => GuildDisbandAck,
    0xB0F2 => GuildLeaveAck,
    0xB0F3 => GuildInviteAck,
    0xB0F4 => GuildKickAck,
    0xB0F6 => GuildDonateAck,
    0xB0F9 => GuildNoticeEditAck,
    0xB0FA => GuildPromoteAck,
    0xB104 => GuildPermissionUpdateAck,

    // The C->S half of the same family (builders in
    // docs/re/net/outbound/guild-union.md, all [V]). Deliberately absent:
    // 0x70F6 (GP donation) — it has a builder but no UI path reaches it in
    // v1.188, so sending it would be inventing a feature, not cloning one.
    0x70F0 => GuildCreateRequest,
    0x70F1 => GuildDisbandRequest,
    0x70F2 => GuildLeaveRequest,
    0x70F4 => GuildKickRequest,
    0x70FA => GuildPromoteRequest,
    0x7104 => GuildPermissionUpdateRequest,

    // Guild union / alliance (see agent/guild_union.rs, docs/net-guild-union.md).
    // The acks are the same body form as the guild cluster above. 0x3102
    // (union roster push) stays unwired: its record was never decoded and the
    // one published reading of it is a handler-VA misattribution.
    0x70FB => UnionInviteRequest,
    0x70FC => UnionLeaveRequest,
    0x70FD => UnionExpelRequest,
    0xB0FB => UnionInviteAck,
    0xB0FC => UnionLeaveAck,
    0xB0FD => UnionExpelAck,

    // Guild leadership: master transfer, the three election opcodes and the GP
    // history (see agent/guild_leadership.rs, docs/net-guild-leadership.md).
    // 0xB106 and 0xB501 are the only guild bodies with a repeating record.
    0x7103 => GuildMasterTransferRequest,
    0x7105 => GuildElectionStartRequest,
    0x7106 => GuildElectionParticipateRequest,
    0x7107 => GuildElectionVoteRequest,
    0x7501 => GuildGpHistoryRequest,
    0xB103 => GuildMasterTransferAck,
    0xB105 => GuildElectionStartAck,
    0xB106 => GuildElectionRoster,
    0xB107 => GuildElectionVoteAck,
    0xB501 => GuildGpHistoryResponse,

    // Guild war & siege authority (see agent/guild_war.rs,
    // docs/net-guild-war.md). 0x3109 GuildWarInfo stays unwired: no capture and
    // no recorded layout. 0x7113 is a bare u32 whose verb nobody has named, so
    // it is documented rather than modelled.
    0x30EF => GuildRelationUpdate,
    0x70FF => SiegeAuthorityUpdateRequest,
    0x7110 => GuildWarStartRequest,
    0x7112 => GuildWarEndRequest,
    0x7114 => GuildWarRewardRequest,
    0xB110 => GuildWarStartAck,
    0xB112 => GuildWarEndAck,
    0xB114 => GuildWarRewardAck,

    // Inventory (see agent/inventory.rs).
    0x7034 => InventoryOperationRequest,
    0xB034 => InventoryOperationResponse,
    0x3036 => PlayerPickupAnimation,
    0x3038 => EntityEquip,
    0x3039 => EntityUnequip,
    0x3040 => InventoryItemUpdate,
    0x3052 => InventoryItemDurabilityUpdate,
    0x3092 => InventoryCapacityUpdate,

    // Item use (0x704C = CLIENT_ITEM_USE per SilkroadDoc). The request body is
    // switched on the item's class — three variants read off the original's two
    // builders (#454); the 0xB04C response is still SPEC-derived and capture
    // group C is pending. See agent/inventory.rs.
    0x704C => ItemUseRequest,
    0xB04C => ItemUseResponse,

    // Storage (capture-verified 2026-08-08/10 — layouts from xBot-WinForms;
    // see agent/storage.rs and docs/net-storage-0x3047-0x3049.md). The push
    // is begin/data/end like CHARACTER_DATA, with the data possibly split
    // across several 0x3049 packets — and it arrives only ONCE per character
    // session; repeat 0x703C requests are refused with a 0xB03C error ack.
    0x703C => StorageDataRequest,
    0xB03C => StorageDataResponse,
    0x3047 => StorageDataBegin,
    0x3049 => StorageDataChunk,
    0x3048 => StorageDataEnd,

    // Guild storage (guild warehouse) — the same begin/data/end push one
    // family over (agent/guild_storage.rs, docs/net-guild-storage-0x7250.md),
    // with one structural difference: the stream is the REPLY to 0x7252,
    // which the original sends itself from the 0xB250 success arm. Bodies are
    // read from the original's builders/handlers (#266); no dump exists for
    // any of the seven, so nothing here is capture-verified.
    0x7250 => GuildStorageOpenRequest,
    0x7251 => GuildStorageCloseRequest,
    0x7252 => GuildStorageListRequest,
    0xB250 => GuildStorageResponse,
    0x3253 => GuildStorageDataBegin,
    0x3255 => GuildStorageDataChunk,
    0x3254 => GuildStorageDataEnd,

    // Alchemy — dismantle only (agent/alchemy.rs, docs/net-alchemy.md).
    // Twenty opcodes in this block, ONE published body: `0x7157 {u8 count,
    // u8[] slots}` -> `0xB157 {u8 result, if result == 2 u16 code}`
    // (docs/re/systems/alchemy.md:44). Elixir 0x7150/0xB150, stone
    // 0x7151/0xB151, manufacture+disjoin 0x7155/0xB155, socket 0x716A/0xB16A,
    // the abort 0x3156 and the six unnamed neighbours have NO recorded layout
    // in any source, so they stay unwired and are listed with their handler
    // VAs in docs/net-alchemy.md rather than guessed at (#757).
    0x7157 => AlchemyDismantleRequest,
    0xB157 => AlchemyDismantleResponse,

    // Battle Arena scheduler broadcast (agent/barena.rs,
    // docs/re/systems/battle-arena.md §3a). Capture-verified for ops
    // 02/03/05/0D/0E from our own packet_dump/0x34d2.log; the body is a tagged
    // union whose length varies per op, so it is typed as {op, body} and read
    // through accessors rather than a fixed derive.
    //
    // The C→S register/cancel request stays unwired: the catalog says 0x74D3
    // and the wiki header says 0x74D2, and picking one without an outbound dump
    // would be inventing the wire (battle-arena.md §9.1).
    0x34D2 => BArenaOperation,

    // Pet / COS wire family (see agent/pet.rs, docs/re/net/{inbound,outbound}/
    // pet-cos.md). Layouts come from the original client's own handlers and
    // builders, cross-checked against the vSRO server's writers where one
    // exists; the older xBot-derived docs/net-pet-0x30C8.md is superseded.
    //
    // 0x30C8's tail layout and 0x30C9 arm 2's item records are selected by
    // loaded refdata (the model's tid4 / the item's itemdata row), which is not
    // in the body, so those parts are read through resolver-taking accessors
    // rather than typed by the derive.
    //
    // CLIENT_PET_DESTROY (0x706C) stays unwired: it collides with
    // PartyMatchListRequest, which already owns that number.
    0x30C8 => PetData,
    0x30C9 => PetUpdate,
    0x30E7 => StuckDistanceWarning,
    0xB0C5 => PetActionResponse,
    0xB0C6 => PetTerminateResponse,
    0xB0CB => PetPlayerMounted,
    0xB116 => PetUnsummonResponse,
    0xB117 => PetRenameResponse,
    0xB420 => PetSettingsChangeResponse,
    0x70C5 => PetActionRequest,
    0x70C6 => PetTerminateRequest,
    0x70CB => PetMountRequest,
    0x7116 => PetUnsummonRequest,
    0x7117 => PetRenameRequest,
    0x7420 => PetSettingsChangeRequest,

    // Mail/memo + consignment "avatar market" (see agent/mail.rs,
    // docs/net-mail-consignment-0x7309.md). Spec-derived: no packet_dump exists
    // for any of these yet. 0xB509's records are variable-length and their width
    // depends on the client's itemdata, so its list stays raw behind a
    // resolver-taking accessor.
    //
    // Three siblings stay unwired because their bodies are entirely unverified —
    // the mail/memo send RESPONSE, and the consignment register/unregister
    // REQUESTS. Wiring them needs packet_dump/0xb309.log, 0x7508.log and
    // 0x7509.log respectively.
    0x750E => ConsignmentListRequest,
    0xB508 => ConsignmentRegisterResponse,
    0xB509 => ConsignmentUnregisterResponse,
    0x7309 => MailSendRequest,

    // Player stall / private shop (see agent/stall.rs, docs/net-stall-0x30B7.md).
    // Spec-derived: no packet_dump exists for any of these yet. The stall listing
    // streams as a 0xFF-sentinel-terminated row list whose rows embed the shared
    // item body, so 0x30B7 and 0xB0BA keep their rows raw behind resolver-taking
    // accessors — no derive list mode can express either half.
    0x70B1 => StallCreateRequest,
    0x70B2 => StallDestroyRequest,
    0x70B4 => StallBuyRequest,
    0x70B5 => StallLeaveRequest,
    0x70BA => StallUpdateRequest,
    0xB0B1 => StallCreateResponse,
    0xB0B2 => StallDestroyResponse,
    0xB0B4 => StallBuyResponse,
    0xB0B5 => StallLeaveResponse,
    0xB0BA => StallUpdateResponse,
    0x30B7 => StallEntityAction,
    0x30B8 => EntityStallCreate,
    0x30B9 => EntityStallDestroy,
    // 0xB0B3 stall-talk snapshot (#759). Its C→S partner 0x70B3 stays unwired:
    // no source on this machine transcribes the request body — see
    // docs/net-stall-0x30B7.md §9.
    0xB0B3 => StallTalkResponse,
    0x30BB => EntityStallTitleUpdate
}

/// A compact hex preview of a packet body, for validating layouts against live
/// captures (used by the client's network diagnostics).
pub fn hexdump(raw: &[u8], max: usize) -> String {
    raw.iter()
        .take(max)
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
