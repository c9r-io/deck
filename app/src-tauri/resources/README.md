# macOS localization resources

`en.lproj` and `zh-Hans.lproj` are the canonical native metadata resources.
The current direct development build does not consume localized Info.plist
values. Before App Store packaging, copy these `.lproj` directories to the
application bundle's `Contents/Resources` directory in the signing pipeline;
do not add fake privacy usage descriptions for capabilities deck does not use.
The root `src-tauri/Info.plist` intentionally contains the real Local Network
usage description: terminal tools hosted by deck may connect to local services
the user selected. Keep that wording accurate and do not broaden it into a
claim that deck scans the network.

App Store listing copy belongs in the release service, not in
`InfoPlist.strings`. English remains the source listing; Simplified Chinese
listing copy should link to `docs/zh-Hans.md` for the maintained terminology.
