# Estado do roadmap

Marco em execução: **v0.2** (fases A0, A1, A2, A3, A7). Plano: `local://naro-global-roadmap-plan.md`.

## Fases

| Fase | Estado | Evidência | Tentativas | Notas |
|---|---|---|---|---|
| A0 | concluída | tag `baseline-f8` + `phase-a0`; `228 passed / 0 failed`; `--compat` 27/0/1; `computed:`/`attr:` verificados em positivo e negativo | 1 | baseline registrado abaixo |
| A1 | concluída | tag `phase-a1`; `228 passed / 0 failed`; `--compat` **28 pass / 0 falhas / 1 não suportado** (29 total); `29-platform.html` com 46 checks; 4 páginas de referência com `JS ERROR` idênticos ao baseline e `CONTENT_HEIGHT` idêntico; `diff reports/p8-rust.txt` = 0 | 1 | ver desvios abaixo |
| A2 | pendente | — | 0 | depende de A1 (concluída) |
| A3 | pendente | — | 0 | depende de A1 (concluída); POST depende de A2 |
| A7 | pendente | — | 0 | depende de A2 e A3 |
| Marco v0.2 | pendente | — | 0 | `reports/milestone-v0.2.md` |

## Próxima ação

A2, passo 1: criar `NetSession` em `crates/naro-net` e migrar os 5 chamadores de
`naro_net::fetch` em `crates/naro-app/src/main.rs`.

## Bloqueios

Nenhum.

## Desvios e decisões de A1

1. `naro-css.workspace = true` **já existia** em `crates/naro-js/Cargo.toml`; o passo 1
   do plano era desnecessário. `url.workspace = true` foi acrescentado a `naro-js`
   para o parse/resolução do `URL` e do `location` usarem as mesmas regras do loader
   (dependência já presente no workspace, não é adição nova).
2. `matchMedia` **avalia** `(min-width|max-width|min-height|max-height: Npx)`, `all`,
   `screen` e `and`, em vez de devolver sempre `matches: false`. Superconjunto do que
   o plano pedia; o resultado é um snapshot (sem reavaliação em resize).
3. `Event.bubbles` é registrado e exposto, mas o despacho continua percorrendo toda a
   cadeia de ancestrais independentemente dele: os eventos sintéticos do motor
   dependem disso e nenhuma página existente passa `bubbles: true`.
4. Dois defeitos encontrados pela página nova e corrigidos:
   - `run_timers` descartava um segundo `setTimeout(fn, 0)` enfileirado no mesmo
     milissegundo do primeiro (`if (t.due >= now) break`). O limite de 100 execuções
     e o orçamento de 50 ms continuam impedindo cadeias infinitas.
   - `text_of` (usado por `textContent`) devolvia os filhos de nível superior em
     ordem invertida.
5. `location.assign/replace/reload` seguem no-op: navegação é do motor. `location`
   passou a ser uma view dinâmica, então `pushState` muda `location.search` na hora;
   o vetor de histórico do motor é atualizado no `flush` seguinte.

## Baseline de medição

Data: 2026-02-13 (sessão de execução do marco v0.2). Toolchain
`+stable-x86_64-pc-windows-gnu`, PATH do MinGW UCRT.

### Testes e compatibilidade

| Comando | Resultado |
|---|---|
| `cargo +stable-x86_64-pc-windows-gnu test --workspace` | 228 passed, 0 failed |
| `./target/release/naro.exe --compat tests/compat` | 27 pass, 0 falhas, 1 não suportado (28) |
| `diff reports/p8-rust.txt` vs dump atual de `https://www.rust-lang.org` | 0 linhas de diferença |

### `--bench <alvo> 10`, mediana de 3 execuções (ms)

| Alvo | parse | css | style | layout | load | alocs (n) |
|---|---|---|---|---|---|---|
| `naro://start` | 0.012 | 0.017 | 0.063 | 0.044 | 4.139 | 16351 |
| `https://example.com` | 0.008 | 0.008 | 0.041 | 0.018 | 6.285 | 7319 |
| `https://www.rust-lang.org` | 0.509 | 3.156 | 2.878 | 1.175 | 20.498 | 413109 |
| `https://developer.mozilla.org/en-US/docs/Web/HTML` | 1.829 | 1.806 | 12.869 | 5.912 | 44.760 | 733343 |
| `https://www.google.com` | 0.844 | 3.950 | 2.343 | 0.481 | 35.213 | 432742 |

`load` é a carga ponta a ponta medida pelo binário (inclui a rede, portanto varia
com a latência; as demais colunas são determinísticas).

### Páginas de referência (`--dump`, 1000 px de largura)

| Página | TITLE | STATUS | CSS | CONTENT_HEIGHT | JS ERROR |
|---|---|---|---|---|---|
| `https://example.com` | Example Domain | Idle | rules=4 invalid=0 | 820.60 | 0 |
| `https://www.rust-lang.org` | Rust Programming Language | Idle | rules=2052 invalid=0 | 3968.98 | 0 |
| `https://developer.mozilla.org/en-US/docs/Web/HTML` | HTML: HyperText Markup Language \| MDN | Idle | rules=427 invalid=10 | 16897.00 | 6 |
| `https://www.google.com` | Google | Idle | rules=1276 invalid=0 | 487.90 | 8 |

Google serve HTML e bundles diferentes a cada carga (`bytes html` variou
212460–242048 no `--bench`, e as URLs dos scripts mudam a cada resposta): as linhas
`JS ERROR` de google.com diferem em URL e em coluna, mas não em classe de erro nem em
contagem. Diagnóstico, não gate.

### Ambiente

- Repositório tinha um único commit (`355338b Initial commit`) e 214 arquivos não
  rastreados; `git status` listado antes do commit: `M README.md`, `?? .tmp/`,
  `?? Cargo.lock`, `?? Cargo.toml`, `?? crates/`, `?? g.html`, `?? g.png`,
  `?? reports/`, `?? tests/`. Nada foi descartado.
- `.gitignore` ganhou `.tmp/` e `reports/*.bmp` (48 BMP, ~97 MB de capturas
  reproduzíveis); os PNG e `.md` de `reports/` continuam versionados.
