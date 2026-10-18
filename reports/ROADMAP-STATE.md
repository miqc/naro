# Estado do roadmap

Marco em execução: **v0.2** (fases A0, A1, A2, A3, A7). Plano: `local://naro-global-roadmap-plan.md`.

## Fases

| Fase | Estado | Evidência | Tentativas | Notas |
|---|---|---|---|---|
| A0 | concluída | tag `baseline-f8`; `228 passed / 0 failed`; `--compat` 27/0/1; checks `computed:`/`attr:` verificados em positivo e negativo | 1 | baseline registrado abaixo |
| A1 | pendente | — | 0 | depende de A0 |
| A2 | pendente | — | 0 | depende de A1 |
| A3 | pendente | — | 0 | depende de A1 (POST depende de A2) |
| A7 | pendente | — | 0 | depende de A2 e A3 |
| Marco v0.2 | pendente | — | 0 | `reports/milestone-v0.2.md` |

## Próxima ação

A1, passo 1: acrescentar `naro-css.workspace = true` a `crates/naro-js/Cargo.toml`.

## Bloqueios

Nenhum.

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

Google serve HTML diferente a cada carga (`bytes html` variou 212460–242048 no
`--bench`); os valores acima são diagnóstico, não gate.

### Ambiente

- Repositório tinha um único commit (`355338b Initial commit`) e 214 arquivos não
  rastreados; `git status` listado antes do commit: `M README.md`, `?? .tmp/`,
  `?? Cargo.lock`, `?? Cargo.toml`, `?? crates/`, `?? g.html`, `?? g.png`,
  `?? reports/`, `?? tests/`. Nada foi descartado.
- `.gitignore` ganhou `.tmp/` e `reports/*.bmp` (48 BMP, ~97 MB de capturas
  reproduzíveis); os PNG e `.md` de `reports/` continuam versionados.
