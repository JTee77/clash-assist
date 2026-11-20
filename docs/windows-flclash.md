# Windows FlClash

macOS FlClash remains the product reference. Windows does not reuse macOS paths or the Verge named pipe.

Evidence is from [chen08209/FlClash](https://github.com/chen08209/FlClash) (`services/helper`, service name `FlClashHelperService`, launch argument `--owner-sid`) and from a real Windows 10 install (OfficeDesktop-HE, FlClash with TUN, Clash Assist 0.3.5 portable at `53efa646`). On that machine the core listened on `127.0.0.1:9090` as `FlClashCore.exe` in Session 0. A medium-integrity process got `OpenProcess` error 5 and an empty WTS `pUserSid`. The NT image was still `\Device\HarddiskVolume3\Program Files\FlClash\FlClashCore.exe`. The helper service ran as LocalSystem with `--owner-sid` of the interactive user.

## Where the files are

`windows/runner/Runner.rc` sets CompanyName `com.follow` and ProductName `clash`. Flutter `path_provider` `getApplicationSupportDirectory` joins the roaming AppData folder with those two names.

`lib/common/path.dart` then puts these files in that directory:

- `shared_preferences.json` (`flutter.config` → `patchClashConfig`)
- `config.yaml`

So the directory is `%APPDATA%\com.follow\clash`. Issues [#1770](https://github.com/chen08209/FlClash/issues/1770) and [#2126](https://github.com/chen08209/FlClash/issues/2126) show the same path. Portable builds use it too ([#1882](https://github.com/chen08209/FlClash/issues/1882)); they do not keep config next to `FlClash.exe`.

macOS stays on `~/Library/Preferences/com.follow.clash.plist` and `~/Library/Application Support/com.follow.clash`.

## How the controller works

On FlClash 0.9.x the external-controller switch is under 工具 → 覆写 → 基础. The listen address is fixed at `127.0.0.1:9090`. The older 设置 → 常规 → 入站 path is not where this build puts the switch. The default is closed. Opening it fills an empty `secret`. The mixed port default used when the file omits one is 7890, same as the macOS fallback.

The core process is `FlClashCore.exe`, beside `FlClash.exe` (`AppPath.corePath`). The external controller listens in that core process. Desktop FlClash talks to the core over its own socket; that socket is not the Mihomo REST API. There is no Verge-style named pipe.

Windows therefore:

1. Reads only the roaming directory above.
2. Treats a missing directory as “FlClash is not installed / has no config”.
3. Treats a closed external controller as “turn it on”. On FlClash 0.9.x that switch is 工具 → 覆写 → 基础.
4. Treats an open controller with nothing listening as “FlClash is not running”.
5. Connects with TCP to `127.0.0.1` only after the listener is this user's `FlClashCore.exe`. A core the user started directly is accepted when its token is the current user. The official helper-service core (Session 0, `OpenProcess` denied) is accepted only when the NT image and the process snapshot are both `FlClashCore.exe`, its parent is the registered `FlClashHelperService.exe` in the same directory, and that service's `--owner-sid` is the current user. The secret is not sent until that check succeeds, and the same check is repeated after the TCP connection is accepted and before the first byte. If the helper proof fails, the log keeps `listener-unverified` and adds a `helper:` reason. That case is not “the controller is not listening”.
6. Sends the secret from those files, and only after that check. The secret is not returned to the webview. A subscription URL in `config.yaml` is not copied into the source label or the discovery log.
7. Accepts a non-default port only when the address is loopback or a wildcard (`127.0.0.1`, `localhost`, `::1`, `0.0.0.0`, `::`). A LAN or public address is not dialed, and that port is not reused on `127.0.0.1`.
8. If Verge's yaml names the same TCP port, the listener decides. `FlClashCore.exe` owned by the current user gets the FlClash secret. `verge-mihomo.exe` stays on the Verge path and does not receive that secret. Any other image or user is refused with no bytes sent. Verge's named-pipe checks and the Verge TCP secret gate are unchanged.

`FlClash.exe` itself is not accepted as the listener. If a future FlClash build moves the core into the UI process, this check will refuse the connection until the allow-list is updated from upstream.

## Limits that remain

- A core the current user starts directly is still accepted by file name from any folder. The Session 0 helper path is stricter: the core and `FlClashHelperService.exe` must be the registered service binary and share its directory, and `--owner-sid` must be the current user.
- When `OpenProcess` succeeds, the process handle is held until the post-connect PID check. When it does not (helper core), the helper proof is repeated after connect and before the first byte.
- Node refresh, switch, test, and restore on a real FlClash install are not yet confirmed. The OfficeDesktop-HE run stopped at this listener check. A build that does not use `%APPDATA%\com.follow\clash` is not detected.
