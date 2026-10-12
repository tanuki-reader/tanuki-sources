# Source candidates (en + pt-BR)

Sources worth porting to Tanuki, taken from the extensions Mihon users run via the
[Keiyoushi repo](https://github.com/keiyoushi/extensions-source). Checked 2026-10-12.

- **Keiyoushi reference** = the extension's folder in `extensions-source`, `src/<lang>/<dir>`. Read it
  for the endpoints, headers, login and Cloudflare handling it needs.
- **Popularity** is community prominence, not measured usage: nobody publishes per-source numbers.
- **Status** comes from fetching each homepage, so a Cloudflare challenge may not show up. Where the
  Keiyoushi code deals with Cloudflare, login or scrambled images, that is noted.
- Adult-only sources are left out.

## Already in Tanuki

MangaDex (en, pt-BR), MangaFire (en, pt-BR), Hipercool (pt-BR), MangaPill (en), Nexus Mangás (pt-BR),
Manhastro (pt-BR, built but not in
`bundles.toml`).

## Manga and manhwa, English

### Easy: up, no login

| Source | Site | Keiyoushi reference | Notes |
|---|---|---|---|
| Weeb Central | weebcentral.com | `src/en/weebcentral` | Plain HTML, rate limited. |
| Mangakakalot | mangakakalot.gg | `src/en/mangakakalot` | HTML. Needs image merging and an alternate CDN. |
| Manganato | natomanga.com | `src/en/manganelo` | Same family as Mangakakalot. |
| Dynasty Scans | dynasty-scans.com | `src/en/dynasty` | Up, rate limited, cover-URL lookup. |
| Flame Comics | flamecomics.xyz | `src/en/flamecomics` | Next.js JSON, build ID changes. Banner reports recent downtime. |
| TCB Scans | tcbonepiecechapters.com | `src/en/tcbscans` | Small catalog (One Piece, Jujutsu Kaisen and similar). |
| Bbato | bato1.com | `src/en/bbato` | Anonymous reading. |
| Webtoons | webtoons.com | `src/all/webtoons` | Official. |
| MANGA Plus | mangaplus.shueisha.co.jp | `src/all/mangaplus` | Official, API based. Needs JavaScript, so I couldn't see more. |

### Needs extra work

| Source | Site | Keiyoushi reference | Notes |
|---|---|---|---|
| Asura Scans | asurascans.com | `src/en/asurascans` | Top manhwa source. Has an API. Premium chapters need a login token. Images scrambled. |
| Comix | comix.to | `src/en/comix` | Cloudflare challenge handled in a WebView. Images need descrambling. |
| Comikey | comikey.com | `src/all/comikey` | Locked chapters unlock through a WebView. Some are paid. |
| Tapas | tapas.io | `src/en/tapastic` | Needs a cookie. Some episodes are paid. |
| Cubari | cubari.moe | `src/all/cubari` | No catalog. It only reads links you give it. |

## Manga and manhwa, pt-BR

### Easy: up, no login

| Source | Site | Keiyoushi reference | Notes |
|---|---|---|---|
| MANGA Plus (pt-BR) | mangaplus.shueisha.co.jp | `src/all/mangaplus` | Same code as the English one. |
| Nexus Toons | nexustoons.com | `src/pt/spectralscan` | Only the page metadata loaded, so check it by hand. |
| Tsundoku Traduções | tsundoku.com.br | `src/pt/tsundokutraducoes` | WordPress (MangaThemesia). |
| Manga Livre.to | mangalivre.to | `src/pt/mangalivreto` | WordPress (Madara). |
| Astratoons | new.astratoons.com | `src/pt/astratoons` | JavaScript app. Login via Discord or Google is optional. |

### Needs extra work

| Source | Site | Keiyoushi reference | Notes |
|---|---|---|---|
| Comikey Brasil | br.comikey.com | `src/all/comikey` | Same as Comikey. |
| Taiyō | taiyo.moe | `src/pt/taiyo` | Up. The extension fetches a bearer token. |
| Lycan Toons | lycantoons.com | `src/pt/lycantoons` | Cloudflare blocks non-browser requests, so everything goes through a WebView. |
| Yomu Mangás | yomumangas.com | `src/pt/yomumangas` | Domain moved to global.yomumangas.com. Returned a 503 when checked. Uses api.yomumangas.com. |

### Skip, or fix first

| Source | Reason | Keiyoushi reference |
|---|---|---|
| Argos Scan | Needs a login. Homepage fetch errored. | `src/pt/argosscan` |
| Mediocre Toons | Needs an email and password. Homepage fetch errored. | `src/pt/mediocretoons` |
| MangaLivre.org | Redirects to noxmangas.org, which looks new or inactive. | `src/pt/mangalivreorg` |

## Western comics and webcomics

Keiyoushi has fewer of these than manga, and most are small.

### English: easy, up, no login

| Source | Site | Keiyoushi reference | Notes |
|---|---|---|---|
| ReadAllComics | readallcomics.com | `src/en/readallcomicscom` | Superhero comics. Up. |
| Comic Fury | comicfury.com | `src/all/comicfury` | Webcomic host. Reading needs no login. Also listed as en and pt-BR. |
| Hiveworks Comics | hiveworkscomics.com | `src/en/hiveworks` | Free webcomics. |
| Comics Kingdom | comicskingdom.com | `src/all/comicskingdom` | Newspaper strips. Not checked. |
| Digital Comic Museum | digitalcomicmuseum.com | `src/en/digitalcomicmuseum` | Public-domain golden age comics. Browsing is open, but downloading asks you to register. |
| Dragon Ball Multiverse | dragonball-multiverse.com | `src/all/dragonballmultiverse` | Fan comic in en and pt-BR. Not checked. |
| Single webcomics | various | `src/en/xkcd`, `explosm` (Cyanide & Happiness), `saturdaymorningbreakfastcomics`, `questionablecontent`, `gunnerkriggcourt`, `megatokyo`, `keenspot` (TwoKinds), `darklegacycomics`, `grrlpower`, `existentialcomics` | One site per extension. Only the Keiyoushi folders were checked, not the sites. |

### English: needs extra work

| Source | Site | Keiyoushi reference | Notes |
|---|---|---|---|
| ReadComicOnline | readcomiconline.li | `src/en/readcomiconline` | Captcha that has to be solved in a WebView. Fetch was refused here. |
| Read Comics Online | readcomicsonline.ru | `src/en/readcomicsonline` | Fetch errored. Check by hand. |
| BatCave | batcave.biz | `src/en/batcave` | Uses a WebView. Fetch errored, but its page text says no registration is needed. |
| GlobalComix | globalcomix.com | `src/all/globalcomix` | Many titles are paid or need a Gold membership. Also en and pt-BR. |
| Webcomics (app) | webcomicsapp.com | `src/all/webcomics` | Has an API. Not checked for paid content. |

### English: needs an account or paid access

| Source | Keiyoushi reference | Notes |
|---|---|---|
| VIZ | `src/en/vizshonenjump` | Needs a login and subscription. |
| INKR | `src/en/inkr` | Coins or subscription for most chapters. |
| Mangamo | `src/en/mangamo` | Login, with a subscription for parts. |
| Madokami | `src/en/madokami` | Username and password. |
| izneo | `src/all/izneo` | Username and password. |
| Kodansha | `src/en/kodansha` | Images are XOR-obfuscated. |

### pt-BR comics (HQs)

| Source | Site | Keiyoushi reference | Notes |
|---|---|---|---|
| Pluma Comics | plumacomics.cloud | `src/pt/plumacomics` | Up. VIP is optional (early and ad-free). |
| HQ Now! | hq-now.com | `src/pt/hqnow` | Brazilian HQ catalog. Needs JavaScript, so I couldn't see more. |
| Geass Comics | geasscomics.xyz | `src/pt/geasscomics` | Not checked. |
| Yomu Comics | yomu.com.br | `src/pt/sssscanlator` | Not checked. |
| Comic Fury, Dragon Ball Multiverse | | `src/all/comicfury`, `src/all/dragonballmultiverse` | The same extensions listed above under English, also available in pt-BR. |

## Suggested order

1. Weeb Central, MANGA Plus (en and pt-BR), Webtoons, Tsundoku, Manga Livre.to.
2. Asura Scans (free chapters only), plus ReadAllComics and Pluma Comics for comics.
3. The Cloudflare and WebView ones (Comix, Lycan Toons, ReadComicOnline).
