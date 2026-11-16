# Changelog

## [0.3.0](https://github.com/Zebradil/scry/compare/v0.2.0...v0.3.0) (2026-11-16)


### Features

* run scry's viewer in the browser on a live demo page ([#30](https://github.com/Zebradil/scry/issues/30)) ([50161bb](https://github.com/Zebradil/scry/commit/50161bb5c7afaf3aaa5da50ad4416d822db77029))
* **web:** draw the viewer with Ratzilla's DOM backend ([#36](https://github.com/Zebradil/scry/issues/36)) ([e74f55b](https://github.com/Zebradil/scry/commit/e74f55b3d9aa7adfb5499f2b75c8f891b0753a03))


### Fixes

* **deps:** update dependency @zebradil/docs-kit to v0.12.0 ([#31](https://github.com/Zebradil/scry/issues/31)) ([e6aeff5](https://github.com/Zebradil/scry/commit/e6aeff58b7ffa298299de9ae020c50a1f835effa))
* **deps:** update docs-kit to v0.11.1 ([#24](https://github.com/Zebradil/scry/issues/24)) ([02b5fcd](https://github.com/Zebradil/scry/commit/02b5fcdbc10585da6d1d12c1629eb0a2a1f5dc52))
* **files:** strip Nix case-hack suffixes from listed names ([#40](https://github.com/Zebradil/scry/issues/40)) ([848b175](https://github.com/Zebradil/scry/commit/848b175cd967dd3600a29fc9f758aa50b4b0c72a))


### Documentation

* add a recorded demo to the README and docs ([#27](https://github.com/Zebradil/scry/issues/27)) ([2bdac70](https://github.com/Zebradil/scry/commit/2bdac700a41888e49eec2e927e51a0c3bfd71c0d))
* add counter to the docs site ([ff913d1](https://github.com/Zebradil/scry/commit/ff913d18785c46f86c6659606912592e27b2b7d2))
* serve the browser demo full screen and theme it ([#35](https://github.com/Zebradil/scry/issues/35)) ([ba4092f](https://github.com/Zebradil/scry/commit/ba4092f9ff1ec32aaf5ce74addbeba85413e89b0))
* use remote site.yaml schema URL ([#39](https://github.com/Zebradil/scry/issues/39)) ([8189305](https://github.com/Zebradil/scry/commit/81893053cc2fb694e4e5dad64084d4e37f1868b8))

## [0.2.0](https://github.com/Zebradil/scry/compare/v0.1.0...v0.2.0) (2026-11-15)


### Features

* acceptance checks on real machines (M9) ([f982b2d](https://github.com/Zebradil/scry/commit/f982b2d3036b757dd065da08145269caf6c290bf))
* announce evaluation steps on stderr ([83fb41e](https://github.com/Zebradil/scry/commit/83fb41ea4181bb4b63ba369f7331651d81874474))
* command seam and ssh transport (M4) ([62ea155](https://github.com/Zebradil/scry/commit/62ea155dc064e7219f91bfdde6d5e25c92925f55))
* fetch pipeline (M5) ([7730af1](https://github.com/Zebradil/scry/commit/7730af1f4a8b6dea30049092a9f463d5dfc78e99))
* files and boot core (M2) ([c01edff](https://github.com/Zebradil/scry/commit/c01edffcf4a871b2dccd86e0e74fe6cec00ce1d5))
* fixture mode, completions and README (M8) ([b9102c7](https://github.com/Zebradil/scry/commit/b9102c7d6d008e698db63137e69291716924944b))
* packages core (M1) ([8ac62ee](https://github.com/Zebradil/scry/commit/8ac62eeaffe79671476817d16f378afd2028f52c))
* report rendering (M3) ([6d2436a](https://github.com/Zebradil/scry/commit/6d2436ada0d9dfe92d6f0f29c265c1c3c6bcc5e6))
* scaffold crate, flake, CLI, report types and command seam ([b286b3d](https://github.com/Zebradil/scry/commit/b286b3d414ff4e1f3338d44be97e101afc726829))
* sources and run (M6) ([c4bc888](https://github.com/Zebradil/scry/commit/c4bc888087301928198a9cb1919378078f240031))
* **store:** add store name and parseDrvName split ([4b9605e](https://github.com/Zebradil/scry/commit/4b9605edd16fbb50532ac01f4c1a44ad09b2f7b7))
* TUI viewer (M7) ([6a03525](https://github.com/Zebradil/scry/commit/6a03525d48332d7f077c80afd42f2c10fa16d64b))


### Performance

* **tui:** cache detail rows across key presses ([56d6a56](https://github.com/Zebradil/scry/commit/56d6a5669f3581f8938a04f842d554f73c01fd16))


### Documentation

* add architecture overview and ADRs 0001-0006 ([c5274b7](https://github.com/Zebradil/scry/commit/c5274b76c4cfa3071261882f470ee8862d51e379))
* add handover, M2 done, wave 3 started ([8ee5674](https://github.com/Zebradil/scry/commit/8ee5674690cf381f541707ebfc3fe2eaa97a1821))
* add roadmap, drop employer assumptions from spec ([7aaba8c](https://github.com/Zebradil/scry/commit/7aaba8c440b10c4ff91529231fc2cdde2540bbe1))
* **adr:** acceptance check 4 with builds and Ctrl-C on Linux ([#16](https://github.com/Zebradil/scry/issues/16)) ([9df13cd](https://github.com/Zebradil/scry/commit/9df13cdf60d92afa8a4fa7f68a7e39de57be18ec))
* **adr:** acceptance check 8 with fish login shell ([#13](https://github.com/Zebradil/scry/issues/13)) ([9300ffc](https://github.com/Zebradil/scry/commit/9300ffc89c3791c943e2c39ad4c5e93f419bd028))
* agent skills setup, rename CONTEXT.md to GLOSSARY.md ([20d216d](https://github.com/Zebradil/scry/commit/20d216d96ef6146d0bdd17f5569ff5ac37daa226))
* bootstrap docs-kit site ([#18](https://github.com/Zebradil/scry/issues/18)) ([d328a4a](https://github.com/Zebradil/scry/commit/d328a4a847d5bd0f67f5a2710b3241d28ebf91c8))
* drop unverified claim from ADR 0001 ([3d2a712](https://github.com/Zebradil/scry/commit/3d2a71272ba630dd027921ba5c957323fc882777))
* follow-ups tracked as GitHub issues ([cd083ca](https://github.com/Zebradil/scry/commit/cd083ca5a606c2931702700c92d7380c1daf155a))
* **handover:** current state after M3 ([b739c0c](https://github.com/Zebradil/scry/commit/b739c0cc2698679956386fb0b7b7f06e45c4a20b))
* **handover:** M6 done, M8 implemented, usage pause ([4b3c83e](https://github.com/Zebradil/scry/commit/4b3c83e7ccc911ed73bb4e30706bda3a2851dac6))
* **handover:** M7 implemented, awaiting review ([0ccc7e8](https://github.com/Zebradil/scry/commit/0ccc7e898ba66d5d925bfe99917af027c11fe267))
* **handover:** M8 fix round, M9 local phase ([12497ba](https://github.com/Zebradil/scry/commit/12497ba8e0b868356ebc0a720cddb7ecf8d90e8c))
* **handover:** record M5 review findings ([f49b39c](https://github.com/Zebradil/scry/commit/f49b39cbcef54c4bbe24701e742110e905a0c929))
* **handover:** resume by scheduled wake-up after a usage reset ([1f17043](https://github.com/Zebradil/scry/commit/1f17043894a3487e94ac1ce99c67052cc54cfb31))
* **handover:** usage-limit policy ([b537aa9](https://github.com/Zebradil/scry/commit/b537aa9bce0d5a639c5d9863224d9a9a99067421))
* initial design ([fc0f004](https://github.com/Zebradil/scry/commit/fc0f004b878999f3f0d9c02a3274c68c07ea25e9))
* M5 and M7 merged, M6 awaiting review fixes ([c468057](https://github.com/Zebradil/scry/commit/c4680575b4fde0a97ab3fb87dd431ffd64f6162b))
* M6 merged, M8 in review ([97e4320](https://github.com/Zebradil/scry/commit/97e432097b049a5b1307f54e04b7d6ec8e0ddb86))
* M8 merged, M9 SSH phase ([bc7edab](https://github.com/Zebradil/scry/commit/bc7edabc522a608711a0eb15953a059059c8a943))
* publish docs of past releases with docs-kit 0.11.0 ([#21](https://github.com/Zebradil/scry/issues/21)) ([edc9a02](https://github.com/Zebradil/scry/commit/edc9a02fd9ae1a5507a27fe01303346a4681aabc))
* **readme:** install from github:zebradil/scry ([562658f](https://github.com/Zebradil/scry/commit/562658f21178e4a1f73f62d4fac6a7a1f0faa048))
* **roadmap:** M0 done, wave 2 started ([29ff185](https://github.com/Zebradil/scry/commit/29ff185511d36f55e0de3f819fad08c09c94472f))
* **roadmap:** M1 done ([7bad1de](https://github.com/Zebradil/scry/commit/7bad1de4d3c23d0843c7a736aa19b9dfc906c7d2))
* **roadmap:** M3 done, M7 started ([38ccb05](https://github.com/Zebradil/scry/commit/38ccb05b47b70db6d8ae0843b902a4bea4d74f0b))
* **roadmap:** M4 done ([f6ce6aa](https://github.com/Zebradil/scry/commit/f6ce6aa3f98b4b8a7b4828ae5a9265f5fad0b64f))
