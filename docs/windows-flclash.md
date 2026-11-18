# Windows FlClash

macOS FlClash remains the product reference. Windows does not reuse macOS paths or the Verge named pipe.

Evidence is from [chen08209/FlClash](https://github.com/chen08209/FlClash) commit `be84736605527034689c95bf27828ff27d912c5e` (default branch at the time of this change) and from user reports on that repository. This was not checked against a running FlClash on a Windows machine.

## Where the files are

`windows/runner/Runner.rc` sets CompanyName `com.follow` and ProductName `clash`. Flutter `path_provider` `getApplicationSupportDirectory` joins the roaming AppData folder with those two names.

`lib/common/path.dart` then puts these files in that directory:

- `shared_preferences.json` (`flutter.config` → `patchClashConfig`)
- `config.yaml`

So the directory is `%APPDATA%\com.follow\clash`. Issues [#1770](https://github.com/chen08209/FlClash/issues/1770) and [#2126](https://github.com/chen08209/FlClash/issues/2126) show the same path. Portable builds use it too ([#1882](https://github.com/chen08209/FlClash/issues/1882)); they do not keep config next to `FlClash.exe`.

macOS stays on `~/Library/Preferences/com.follow.clash.plist` and `~/Library/Application Support/com.follow.clash`.

## How the controller works

`ExternalControllerStatus` is either closed (`""`) or `127.0.0.1:9090`. The default is closed. Opening it fills an empty `secret` (`PatchClashConfig.ensureControllerSecret`). The mixed port default used when the file omits one is 7890, same as the macOS fallback.

The core process is `FlClashCore.exe`, beside `FlClash.exe` (`AppPath.corePath`). The external controller listens in that core process. Desktop FlClash talks to the core over its own socket; that socket is not the Mihomo REST API. There is no Verge-style named pipe.

Windows therefore:

1. Reads only the roaming directory above.
2. Treats a missing directory as “FlClash is not installed / has no config”.
3. Treats a closed external controller as “turn it on under Settings → General → Inbound”.
4. Treats an open controller with nothing listening as “FlClash is not running”.
5. Connects with TCP to `127.0.0.1` only after the listener is the current user’s `FlClashCore.exe`.
6. Sends the secret from those files, and only after that check. The secret is not returned to the webview. A subscription URL in `config.yaml` is not copied into the source label or the discovery log.
7. Does not use this path when Verge’s yaml explicitly claims the same TCP port, and does not add `FlClashCore.exe` to the Verge image list. Verge’s named-pipe checks and the Verge TCP secret gate are unchanged.

`FlClash.exe` itself is not accepted as the listener. If a future FlClash build moves the core into the UI process, this check will refuse the connection until the allow-list is updated from upstream.
