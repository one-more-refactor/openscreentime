//! The built-in app & category catalog — the "block YouTube with one click"
//! list. Single source of truth: the server serves it as JSON (`GET
//! /api/catalog`) so the console never duplicates it, and the agent expands a
//! policy's `blocks` through it into concrete DNS sinkholes and process names.
//!
//! Design notes:
//! - Domains are blocked **with all their subdomains** (dnsmasq `address=/d/`),
//!   so `youtube.com` covers `m.youtube.com`; we still list the CDN / API
//!   domains an app actually needs, because blocking the brand domain alone
//!   leaves the app working in practice.
//! - Process names are `comm` names (the first 15 bytes of the executable
//!   name), matched exactly. Only listed where an app has a real native Linux
//!   client; never something generic like `java` or `python3`.
//! - A category is its own domains **plus** every app filed under it.

use crate::AppBlocks;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy)]
pub struct AppDef {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub domains: &'static [&'static str],
    pub processes: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub struct CategoryDef {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// Domains that belong to the category but not to a named app.
    pub domains: &'static [&'static str],
}

const CATEGORIES: &[CategoryDef] = &[
    CategoryDef {
        id: "social",
        name: "Social media",
        blurb: "Feeds, stories, likes.",
        domains: &[
            "threads.net",
            "tumblr.com",
            "vk.com",
            "bereal.com",
            "mastodon.social",
            "bsky.app",
        ],
    },
    CategoryDef {
        id: "video_streaming",
        name: "Video & streaming",
        blurb: "Shows, films, live streams.",
        domains: &[
            "dailymotion.com",
            "vimeo.com",
            "hulu.com",
            "max.com",
            "paramountplus.com",
            "peacocktv.com",
            "crunchyroll.com",
            "kick.com",
            "rumble.com",
            "joyn.de",
            "rtlplus.de",
            "zdf.de",
            "ardmediathek.de",
        ],
    },
    CategoryDef {
        id: "games",
        name: "Games",
        blurb: "Game launchers, stores and servers.",
        domains: &[
            "itch.io",
            "gog.com",
            "battle.net",
            "blizzard.com",
            "ubisoft.com",
            "ubi.com",
            "nintendo.com",
            "nintendo.net",
            "poki.com",
            "crazygames.com",
            "friv.com",
            "y8.com",
            "miniclip.com",
            "agar.io",
            "slither.io",
            "krunker.io",
            "coolmathgames.com",
        ],
    },
    CategoryDef {
        id: "messaging",
        name: "Chat & messaging",
        blurb: "Messengers and group chats.",
        domains: &[
            "viber.com",
            "line.me",
            "wechat.com",
            "kik.com",
            "groupme.com",
            "skype.com",
            "threema.ch",
            "element.io",
            "matrix.org",
        ],
    },
    CategoryDef {
        id: "adult",
        name: "Adult content",
        blurb: "Pornography and explicit sites.",
        domains: &[
            "pornhub.com",
            "xvideos.com",
            "xnxx.com",
            "xhamster.com",
            "redtube.com",
            "youporn.com",
            "onlyfans.com",
            "fansly.com",
            "chaturbate.com",
            "stripchat.com",
            "livejasmin.com",
            "brazzers.com",
            "spankbang.com",
            "eporner.com",
            "tnaflix.com",
            "porn.com",
            "rule34.xxx",
            "e621.net",
            "nhentai.net",
            "hentaihaven.xxx",
            "fapello.com",
            "erome.com",
            "motherless.com",
        ],
    },
    CategoryDef {
        id: "gambling",
        name: "Gambling & betting",
        blurb: "Casinos, betting, loot-box sites.",
        domains: &[
            "bet365.com",
            "stake.com",
            "stake.us",
            "roobet.com",
            "betway.com",
            "pokerstars.com",
            "draftkings.com",
            "fanduel.com",
            "tipico.de",
            "bwin.com",
            "888casino.com",
            "unibet.com",
            "betfair.com",
            "williamhill.com",
            "csgoroll.com",
            "csgoempire.com",
            "rollbit.com",
            "duelbits.com",
            "gamdom.com",
            "lottoland.com",
        ],
    },
    CategoryDef {
        id: "dating",
        name: "Dating",
        blurb: "Dating and hook-up apps.",
        domains: &[
            "match.com",
            "okcupid.com",
            "hinge.co",
            "grindr.com",
            "badoo.com",
            "plentyoffish.com",
            "pof.com",
            "lovoo.com",
            "parship.de",
            "elitepartner.de",
            "zoosk.com",
        ],
    },
    CategoryDef {
        id: "shopping",
        name: "Shopping",
        blurb: "Online stores and marketplaces.",
        domains: &[
            "aliexpress.com",
            "wish.com",
            "etsy.com",
            "zalando.de",
            "zalando.com",
            "otto.de",
            "kleinanzeigen.de",
            "vinted.com",
            "vinted.de",
            "ebay.com",
            "ebay.de",
            "alibaba.com",
        ],
    },
    CategoryDef {
        id: "ai_chat",
        name: "AI chatbots",
        blurb: "Chatbots and AI companions.",
        domains: &[
            "claude.ai",
            "anthropic.com",
            "gemini.google.com",
            "copilot.microsoft.com",
            "perplexity.ai",
            "poe.com",
            "you.com",
            "replika.com",
            "janitorai.com",
            "chai-research.com",
            "mistral.ai",
            "deepseek.com",
        ],
    },
    CategoryDef {
        id: "proxies",
        name: "VPNs, proxies & piracy",
        blurb: "Ways around the rules, and torrent sites.",
        domains: &[
            "croxyproxy.com",
            "proxysite.com",
            "kproxy.com",
            "hidester.com",
            "4everproxy.com",
            "hide.me",
            "vpnbook.com",
            "whoer.net",
            "protonvpn.com",
            "nordvpn.com",
            "expressvpn.com",
            "surfshark.com",
            "windscribe.com",
            "mullvad.net",
            "torproject.org",
            "thepiratebay.org",
            "1337x.to",
            "torrentz2.eu",
            "rarbg.to",
            "yts.mx",
            "nyaa.si",
            "fmovies.to",
            "soap2day.to",
            "123movies.to",
        ],
    },
];

const APPS: &[AppDef] = &[
    // ---- social --------------------------------------------------------------
    AppDef {
        id: "youtube",
        name: "YouTube",
        category: "video_streaming",
        domains: &[
            "youtube.com",
            "youtu.be",
            "googlevideo.com",
            "ytimg.com",
            "youtube-nocookie.com",
            "youtubei.googleapis.com",
            "yt3.ggpht.com",
            "youtubekids.com",
        ],
        processes: &["freetube", "yt-dlp"],
    },
    AppDef {
        id: "tiktok",
        name: "TikTok",
        category: "social",
        domains: &[
            "tiktok.com",
            "tiktokcdn.com",
            "tiktokv.com",
            "tiktokcdn-us.com",
            "musical.ly",
            "byteoversea.com",
            "ibytedtos.com",
            "tiktokv.us",
        ],
        processes: &[],
    },
    AppDef {
        id: "instagram",
        name: "Instagram",
        category: "social",
        domains: &["instagram.com", "cdninstagram.com", "ig.me"],
        processes: &[],
    },
    AppDef {
        id: "snapchat",
        name: "Snapchat",
        category: "social",
        domains: &[
            "snapchat.com",
            "sc-cdn.net",
            "snap.com",
            "snapkit.com",
            "sc-gw.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "facebook",
        name: "Facebook",
        category: "social",
        domains: &[
            "facebook.com",
            "fb.com",
            "fbcdn.net",
            "facebook.net",
            "fbsbx.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "x",
        name: "X (Twitter)",
        category: "social",
        domains: &["x.com", "twitter.com", "twimg.com", "t.co"],
        processes: &[],
    },
    AppDef {
        id: "reddit",
        name: "Reddit",
        category: "social",
        domains: &[
            "reddit.com",
            "redd.it",
            "redditmedia.com",
            "redditstatic.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "pinterest",
        name: "Pinterest",
        category: "social",
        domains: &["pinterest.com", "pinterest.de", "pinimg.com"],
        processes: &[],
    },
    // ---- messaging -----------------------------------------------------------
    AppDef {
        id: "discord",
        name: "Discord",
        category: "messaging",
        domains: &[
            "discord.com",
            "discord.gg",
            "discordapp.com",
            "discordapp.net",
            "discord.media",
            "discord.dev",
        ],
        processes: &[
            "Discord",
            "discord",
            "DiscordCanary",
            "vesktop",
            "armcord",
            "webcord",
        ],
    },
    AppDef {
        id: "whatsapp",
        name: "WhatsApp",
        category: "messaging",
        domains: &["whatsapp.com", "whatsapp.net", "wa.me"],
        processes: &["whatsapp-for-linux", "whatsie", "zapzap"],
    },
    AppDef {
        id: "telegram",
        name: "Telegram",
        category: "messaging",
        domains: &[
            "telegram.org",
            "t.me",
            "telegram.me",
            "telesco.pe",
            "tdesktop.com",
        ],
        processes: &["telegram-desktop", "Telegram"],
    },
    AppDef {
        id: "signal",
        name: "Signal",
        category: "messaging",
        domains: &["signal.org", "whispersystems.org"],
        processes: &["signal-desktop"],
    },
    AppDef {
        id: "messenger",
        name: "Messenger",
        category: "messaging",
        domains: &["messenger.com"],
        processes: &["caprine"],
    },
    AppDef {
        id: "omegle",
        name: "Omegle & stranger chat",
        category: "messaging",
        domains: &[
            "omegle.com",
            "chatroulette.com",
            "ome.tv",
            "emeraldchat.com",
            "monkey.app",
            "chathub.cam",
            "shagle.com",
        ],
        processes: &[],
    },
    // ---- video & streaming -----------------------------------------------------
    AppDef {
        id: "twitch",
        name: "Twitch",
        category: "video_streaming",
        domains: &[
            "twitch.tv",
            "ttvnw.net",
            "jtvnw.net",
            "twitchcdn.net",
            "twitchsvc.net",
        ],
        processes: &["streamlink"],
    },
    AppDef {
        id: "netflix",
        name: "Netflix",
        category: "video_streaming",
        domains: &[
            "netflix.com",
            "nflxvideo.net",
            "nflximg.net",
            "nflxext.com",
            "nflxso.net",
        ],
        processes: &[],
    },
    AppDef {
        id: "disney_plus",
        name: "Disney+",
        category: "video_streaming",
        domains: &[
            "disneyplus.com",
            "disney-plus.net",
            "dssott.com",
            "bamgrid.com",
            "disneystreaming.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "prime_video",
        name: "Prime Video",
        category: "video_streaming",
        domains: &[
            "primevideo.com",
            "aiv-cdn.net",
            "aiv-delivery.net",
            "amazonvideo.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "spotify",
        name: "Spotify",
        category: "video_streaming",
        domains: &["spotify.com", "scdn.co", "spotifycdn.com", "spotifycdn.net"],
        processes: &["spotify"],
    },
    // ---- games ---------------------------------------------------------------
    AppDef {
        id: "roblox",
        name: "Roblox",
        category: "games",
        domains: &["roblox.com", "rbxcdn.com", "rbx.com", "robloxlabs.com"],
        processes: &["sober", "vinegar", "RobloxPlayerBeta"],
    },
    AppDef {
        id: "fortnite",
        name: "Fortnite / Epic",
        category: "games",
        domains: &[
            "epicgames.com",
            "fortnite.com",
            "epicgames.dev",
            "unrealengine.com",
            "ol.epicgames.com",
        ],
        processes: &["heroic", "legendary"],
    },
    AppDef {
        id: "minecraft",
        name: "Minecraft",
        category: "games",
        domains: &[
            "minecraft.net",
            "mojang.com",
            "minecraftservices.com",
            "minecraft-services.net",
            "hypixel.net",
            "curseforge.com",
            "modrinth.com",
        ],
        processes: &[
            "minecraft-launcher",
            "prismlauncher",
            "PrismLauncher",
            "polymc",
            "multimc",
            "atlauncher",
            "ATLauncher",
            "tlauncher",
        ],
    },
    AppDef {
        id: "steam",
        name: "Steam",
        category: "games",
        domains: &[
            "steampowered.com",
            "steamcommunity.com",
            "steamstatic.com",
            "steamcontent.com",
            "steamusercontent.com",
            "steamserver.net",
            "steam-chat.com",
            "valvesoftware.com",
        ],
        processes: &["steam", "steamwebhelper", "steam.sh"],
    },
    AppDef {
        id: "riot",
        name: "League / Valorant",
        category: "games",
        domains: &[
            "riotgames.com",
            "leagueoflegends.com",
            "valorant.com",
            "riotcdn.net",
            "rdatasrv.net",
        ],
        processes: &[
            "LeagueClient",
            "RiotClientServi",
            "RiotClientUx",
            "VALORANT",
        ],
    },
    AppDef {
        id: "ea",
        name: "EA (FIFA, Sims)",
        category: "games",
        domains: &["ea.com", "origin.com", "eaassets-a.akamaihd.net", "ea.net"],
        processes: &["EADesktop", "Origin"],
    },
    AppDef {
        id: "supercell",
        name: "Brawl Stars / Clash",
        category: "games",
        domains: &[
            "supercell.com",
            "brawlstars.com",
            "clashroyale.com",
            "clashofclans.com",
            "supercellgames.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "playstation",
        name: "PlayStation Network",
        category: "games",
        domains: &[
            "playstation.com",
            "playstation.net",
            "sonyentertainmentnetwork.com",
            "playstationnetwork.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "xbox",
        name: "Xbox Live",
        category: "games",
        domains: &["xbox.com", "xboxlive.com", "xboxservices.com"],
        processes: &[],
    },
    AppDef {
        id: "among_us",
        name: "Among Us",
        category: "games",
        domains: &["innersloth.com", "amongus.com"],
        processes: &["Among Us"],
    },
    // ---- AI ------------------------------------------------------------------
    AppDef {
        id: "chatgpt",
        name: "ChatGPT",
        category: "ai_chat",
        domains: &[
            "chatgpt.com",
            "openai.com",
            "oaistatic.com",
            "oaiusercontent.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "character_ai",
        name: "Character.AI",
        category: "ai_chat",
        domains: &["character.ai", "c.ai", "characterai.io"],
        processes: &[],
    },
    // ---- dating / shopping ---------------------------------------------------
    AppDef {
        id: "tinder",
        name: "Tinder",
        category: "dating",
        domains: &["tinder.com", "gotinder.com"],
        processes: &[],
    },
    AppDef {
        id: "bumble",
        name: "Bumble",
        category: "dating",
        domains: &["bumble.com", "bumbcdn.com"],
        processes: &[],
    },
    AppDef {
        id: "amazon",
        name: "Amazon",
        category: "shopping",
        domains: &[
            "amazon.com",
            "amazon.de",
            "amazon.co.uk",
            "amazon.fr",
            "amazon.it",
            "amazon.es",
            "media-amazon.com",
            "amazon-adsystem.com",
        ],
        processes: &[],
    },
    AppDef {
        id: "temu",
        name: "Temu",
        category: "shopping",
        domains: &["temu.com", "kwcdn.com"],
        processes: &[],
    },
    AppDef {
        id: "shein",
        name: "Shein",
        category: "shopping",
        domains: &["shein.com", "shein.de", "sheincorp.com", "ltwebstatic.com"],
        processes: &[],
    },
];

/// Every app in the catalog, in display order.
pub fn apps() -> &'static [AppDef] {
    APPS
}

/// Every category, in display order.
pub fn categories() -> &'static [CategoryDef] {
    CATEGORIES
}

pub fn app(id: &str) -> Option<&'static AppDef> {
    APPS.iter().find(|a| a.id == id)
}

pub fn category(id: &str) -> Option<&'static CategoryDef> {
    CATEGORIES.iter().find(|c| c.id == id)
}

/// The concrete things the device enforces for a set of blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expanded {
    /// Domains to sinkhole (subdomains included). Sorted, de-duplicated,
    /// lower-case, no leading dots.
    pub domains: Vec<String>,
    /// `comm` names to deny for the blocked user. Sorted, de-duplicated.
    pub processes: Vec<String>,
    /// App ids that ended up blocked (directly or via a category) — what the
    /// device may tell the person ("YouTube is blocked").
    pub apps: Vec<String>,
}

fn clean_domain(d: &str) -> Option<String> {
    let d = d
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if d.is_empty() || d.contains('/') || d.contains(char::is_whitespace) {
        return None;
    }
    // Only hostname characters — this string is interpolated into the
    // resolver config on the device.
    if !d
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
    {
        return None;
    }
    Some(d)
}

fn take_app(
    a: &AppDef,
    domains: &mut BTreeSet<String>,
    processes: &mut BTreeSet<String>,
    apps: &mut BTreeSet<String>,
) {
    apps.insert(a.id.to_string());
    for d in a.domains {
        if let Some(d) = clean_domain(d) {
            domains.insert(d);
        }
    }
    for p in a.processes {
        processes.insert((*p).to_string());
    }
}

/// Expand blocks into domains / processes / app ids. Unknown ids are ignored.
pub fn expand(b: &AppBlocks) -> Expanded {
    let mut domains = BTreeSet::new();
    let mut processes = BTreeSet::new();
    let mut apps = BTreeSet::new();

    for id in &b.categories {
        if let Some(c) = category(id) {
            for d in c.domains {
                if let Some(d) = clean_domain(d) {
                    domains.insert(d);
                }
            }
            for a in APPS.iter().filter(|a| a.category == c.id) {
                take_app(a, &mut domains, &mut processes, &mut apps);
            }
        }
    }
    for id in &b.apps {
        if let Some(a) = app(id) {
            take_app(a, &mut domains, &mut processes, &mut apps);
        }
    }
    for d in &b.custom_domains {
        if let Some(d) = clean_domain(d) {
            domains.insert(d);
        }
    }

    Expanded {
        domains: domains.into_iter().collect(),
        processes: processes.into_iter().collect(),
        apps: apps.into_iter().collect(),
    }
}

/// The catalog as JSON for `GET /api/catalog`:
/// `{ categories: [{id,name,blurb,app_ids}], apps: [{id,name,category}] }`.
/// Domain lists are deliberately not shipped to the browser — the console
/// shows names, the device enforces domains.
pub fn as_json() -> Value {
    json!({
        "categories": CATEGORIES.iter().map(|c| json!({
            "id": c.id,
            "name": c.name,
            "blurb": c.blurb,
            "app_ids": APPS.iter().filter(|a| a.category == c.id).map(|a| a.id).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "apps": APPS.iter().map(|a| json!({
            "id": a.id,
            "name": a.name,
            "category": a.category,
            "has_native_client": !a.processes.is_empty(),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_has_a_known_category_and_unique_id() {
        let mut seen = BTreeSet::new();
        for a in APPS {
            assert!(
                category(a.category).is_some(),
                "{} has unknown category {}",
                a.id,
                a.category
            );
            assert!(seen.insert(a.id), "duplicate app id {}", a.id);
            assert!(!a.domains.is_empty(), "{} has no domains", a.id);
            for d in a.domains {
                assert!(clean_domain(d).is_some(), "bad domain {d} in {}", a.id);
            }
        }
        let mut seen = BTreeSet::new();
        for c in CATEGORIES {
            assert!(seen.insert(c.id), "duplicate category id {}", c.id);
        }
    }

    #[test]
    fn expand_unions_categories_apps_and_custom() {
        let b = AppBlocks {
            apps: vec!["youtube".into(), "nope".into()],
            categories: vec!["adult".into()],
            custom_domains: vec![
                " .Example.ORG. ".into(),
                "bad domain".into(),
                "evil.com/x".into(),
            ],
        };
        let e = expand(&b);
        assert!(e.domains.contains(&"youtube.com".to_string()));
        assert!(e.domains.contains(&"googlevideo.com".to_string()));
        assert!(e.domains.contains(&"pornhub.com".to_string()));
        assert!(e.domains.contains(&"example.org".to_string()));
        assert!(!e.domains.iter().any(|d| d.contains(' ') || d.contains('/')));
        assert_eq!(e.apps, vec!["youtube".to_string()]);
        assert!(e.processes.contains(&"freetube".to_string()));
    }

    #[test]
    fn category_pulls_in_its_apps() {
        let b = AppBlocks {
            categories: vec!["messaging".into()],
            ..Default::default()
        };
        let e = expand(&b);
        assert!(e.apps.contains(&"discord".to_string()));
        assert!(e.processes.contains(&"Discord".to_string()));
        assert!(e.domains.contains(&"skype.com".to_string()));
    }

    #[test]
    fn json_shape_has_no_domains() {
        let j = as_json();
        assert!(j["apps"].as_array().unwrap().len() > 20);
        assert!(j["categories"].as_array().unwrap().len() >= 10);
        assert!(j.to_string().find("googlevideo").is_none());
    }
}
