/**
 * Universal egress diagnostics — public surface.
 *
 * The implementation is split into cohesive submodules under `diagnostics/`:
 *   probe (base fetch / reachability / retry / deadline / pool),
 *   exitIp, dns, ipv6, webrtc, aiUnlock, streamingUnlock,
 *   performance (latency + bandwidth), routing (split + bare).
 *
 * This barrel re-exports exactly the public API consumed by nodeRun / envRun /
 * gate and by `lib/egress/index.ts`. Internal helpers stay module-private.
 * Prefer Rust mixed-port proxy fetch (same path as Gemini/IP) so probes follow
 * Clash Verge Rev even when the Tauri WebView does not use system proxy/TUN.
 */

export type { ReachabilityOptions } from "./probe";
export {
  checkReachability,
  withFailRetry,
  withFailRetryUnlock,
  timeoutCard,
  withCardDeadline,
} from "./probe";
export { inferHosting, fetchExitIp, exitIpCard, fetchIpCountry } from "./exitIp";
export { checkDnsResolvers, checkDnsLeakApproach } from "./dns";
export { checkIpv6Leak, probeDirectV4, probeV4Via } from "./ipv6";
export { checkWebRtcLeak, webrtcCard } from "./webrtc";
export {
  probeGeminiUnlock,
  probeChatgptUnlock,
  extractGeminiRegion,
  unlockCard,
} from "./aiUnlock";
export type { BandwidthSampleOptions } from "./performance";
export { sampleLatency, sampleBandwidth } from "./performance";
export { checkSplitRouting, checkBareEgress } from "./routing";
export {
  checkNetflixUnlock,
  checkDisneyUnlock,
  checkYoutubeUnlock,
  checkTikTokUnlock,
  checkSpotifyUnlock,
  checkPrimeVideoUnlock,
  checkAppStoreUnlock,
  checkGooglePlayUnlock,
} from "./streamingUnlock";
