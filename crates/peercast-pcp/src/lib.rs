//! PeerCast PCP protocol wire framing, handshake state machines, and message codecs.

pub mod http_prelude;
pub mod relay_client;
pub mod relay_server;
pub mod probe;
pub mod packet;
pub mod yellow_page;

pub use http_prelude::{
    parse_relay_request, parse_relay_response, PcpHttpError, PcpHttpLimits, PcpHttpStatus,
    PcpRelayRequest, PcpRelayResponse,
};
pub use relay_client::{
    MAX_CANDIDATE_HOSTS, RelayClientPhase, RelayClientStateMachine, RelayError, RelayEvent,
    RelayNetworkProfile,
};
pub use relay_server::{
    RelayServerEvent, RelayServerError, RelayServerPhase, RelayServerStateMachine,
};
pub use probe::{
    execute_reverse_probe, ProbeCodec, ProbeError, ProbeGuard, ProbeLimiter, ProbeLimits,
    ProbeResult,
};
pub use packet::{
    unwrap_wire_pos, ChannelPacket, ChannelPacketAdapter, PacketError, PacketIngestResult,
    PacketPayload, DEFAULT_MAX_HEADER_LEN, MAX_PACKET_DATALEN,
};
pub use yellow_page::{
    create_station_yp_bcst_atom, MockYellowPageServer, PCPYellowPageClient,
    YpAnnouncementChannel, YpBackoffTracker, YpClientConfig, YpConnectionStatus, YpError,
    DEFAULT_SERVANT_VERSION, DEFAULT_YP_UPDATE_INTERVAL, INITIAL_RETRY_DELAY, MAX_RETRY_DELAY,
};
