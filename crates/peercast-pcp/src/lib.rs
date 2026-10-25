//! PeerCast PCP protocol wire framing, handshake state machines, and message codecs.

pub mod http_prelude;
pub mod packet;
pub mod probe;
pub mod relay_client;
pub mod relay_server;
pub mod yellow_page;

pub use http_prelude::{
    parse_relay_request, parse_relay_response, PcpHttpError, PcpHttpLimits, PcpHttpStatus,
    PcpRelayRequest, PcpRelayResponse,
};
pub use packet::{
    unwrap_wire_pos, ChannelPacket, ChannelPacketAdapter, PacketError, PacketIngestResult,
    PacketPayload, DEFAULT_MAX_HEADER_LEN, MAX_PACKET_DATALEN,
};
pub use probe::{
    execute_reverse_probe, execute_reverse_probe_with_profile, ProbeCodec, ProbeError, ProbeGuard,
    ProbeLimiter, ProbeLimits, ProbePeerProfile, ProbeResult,
};
pub use relay_client::{
    RelayClientPhase, RelayClientStateMachine, RelayError, RelayEvent, RelayNetworkProfile,
    MAX_CANDIDATE_HOSTS,
};
pub use relay_server::{
    RelayServerError, RelayServerEvent, RelayServerPhase, RelayServerStateMachine,
};
pub use yellow_page::{
    create_station_yp_bcst_atom, MockYellowPageServer, PCPYellowPageClient, YpAnnouncementChannel,
    YpBackoffTracker, YpClientConfig, YpConnectionStatus, YpError, DEFAULT_SERVANT_VERSION,
    DEFAULT_YP_UPDATE_INTERVAL, INITIAL_RETRY_DELAY, MAX_RETRY_DELAY,
};
