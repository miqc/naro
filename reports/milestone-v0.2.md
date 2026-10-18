# Marco v0.2 — navegador utilizável

Execução autônoma das fases A0, A1, A2, A3 e A7 do roadmap global. Nenhum motor
externo: o NARO continua sendo parser, cascata, layout, pintura e runtime próprios.

## Resultado

| Medida | Baseline (A0) | Marco v0.2 |
|---|---|---|
| `cargo test --workspace` | 228 passed | **235 passed, 0 failed** |
| `--compat` (file://) | 27 pass / 0 fail / 1 unsup (28) | **30 pass / 0 fail / 3 unsup (33)** |
| `--compat --serve` (http) | não existia | **32 pass / 0 fail / 1 unsup (33)** |
| APIs de plataforma ausentes | 22 | 0 das listadas abaixo |
| `p8-rust` (geometria) | referência | 0 diferenças contra `reports/p8-rust-a3.txt` |
| `JS ERROR` nas 4 páginas de referência | 0/0/6/8 | 0/0/6/8 (idêntico) |

Os 3 não suportados em `file://` são as páginas `14-form.html` (digitação exige
`--form`) e as duas que precisam de origem HTTP (`30-net.html`, `33-origin.html`).
Com `--serve`, só `14-form.html` continua fora.

## Matriz de capacidade

### HTML e CSS

| Capacidade | Estado |
|---|---|
| Tokenizador e árvore em arena, `Document`/`Element`/`Text`/`ShadowRoot`/`Fragment` | funciona |
| Seletores: combinadores, atributos, `:nth-child`, `:not`, `:host`, `:root`, `:first-child` | funciona |
| `:checked` (`input[checked]`, `option[selected]`) | funciona (novo em A3) |
| `:hover`/`:focus`/`:active`/`:visited`/`:disabled` | parseado, nunca casa (pintura estática) |
| Cascata de 5 níveis, `!important`, unidades, `calc/min/max/clamp`, `@media` | funciona |
| Folhas externas por inlining no fetch (12 folhas / 512 KB / 3 MB) | funciona |
| `@import`, `@font-face`, `@supports`, `@container` | não implementado |
| `--compat` reporta regras aceitas e descartadas por motivo | funciona |

### Layout

| Capacidade | Estado |
|---|---|
| block, inline, inline-block, contents, flex, grid, tabelas (colspan/rowspan) | funciona |
| `position: relative/absolute/fixed`, `overflow`, `min/max-*` | funciona |
| Controles de formulário como caixa inline (`input`, `button`, `textarea`, `select`) | funciona (`select` novo em A3) |
| `checkbox`/`radio` 13x13 com marca desenhada | funciona (novo em A3) |
| `float`, `vertical-align`, `border-collapse`, `order`, `grid-area` | não implementado (A5c) |

### Pintura

| Capacidade | Estado |
|---|---|
| CPU (softbuffer), retângulos, bordas, AA em paths, arcos, round-rect, SVG inline, imagens, texto | funciona |
| Marca de checkbox (traço) e de radio (ponto) | funciona (novo em A3) |
| Realce de seleção de texto | funciona (novo em A3) |
| Gradientes, `box-shadow` completo, `border-radius` percentual | não implementado (A5a) |
| `transform`, `object-fit`, `aspect-ratio` | não implementado (A5b) |

### JavaScript

| Capacidade | Estado |
|---|---|
| QuickJS; DOM com identidade (`WeakRef`), captura/bubbling, timers, microtasks | funciona |
| `MutationObserver`, `dataset`, medição (`getBoundingClientRect`/`offset*`/`client*`) | funciona |
| Shadow DOM (open/closed), Custom Elements (`connected`/`disconnected`) | funciona |
| `DocumentFragment` (`createDocumentFragment`, `appendChild` que esvazia) | funciona (novo em A1) |
| `getComputedStyle` (getters camelCase, `getPropertyValue`, `cssText`) | funciona (novo em A1) |
| `Event`, `CustomEvent`, `UIEvent`, `MouseEvent`, `KeyboardEvent`, `FocusEvent`, `InputEvent` | funciona (novo em A1) |
| `nextElementSibling`, `firstElementChild`, `childElementCount`, `getElementsByClassName/TagName` | funciona (novo em A1) |
| `cloneNode`, `replaceChild`, `outerHTML` (get/set), `insertAdjacentText`, `hasAttribute`, `toggleAttribute` | funciona (novo em A1) |
| `URL`, `URLSearchParams`, `TextEncoder/Decoder`, `atob`/`btoa`, `structuredClone` | funciona (novo em A1) |
| `history.pushState/replaceState/back/forward/go` + `popstate` | funciona (novo em A1) |
| `document.readyState`, `matchMedia` (largura/altura, `and`) | funciona (novo em A1) |
| `fetch`, `Headers`, `Response`, `Request` | funciona, **síncrono por baixo** (novo em A2) |
| `XMLHttpRequest` (`open`/`send`/`setRequestHeader`/`onload`/`readyState`) | funciona, síncrono (novo em A2) |
| `document.cookie` (leitura e escrita, `HttpOnly` oculto) | funciona (novo em A2) |
| `getSelection().toString()` | funciona (novo em A3) |
| `form.submit()`, `form.reset()`, `form.elements`, `select.options/selectedIndex/value` | funciona (novo em A3) |
| Promise nativa (job queue do QuickJS drenada) | funciona (corrigido em A2) |

### Rede

| Capacidade | Estado |
|---|---|
| `NetSession` de processo: keep-alive, cache e cookies sobrevivem à navegação | funciona (novo em A2) |
| `HttpCache` em memória (32 MB / 200 entradas, FIFO) + revalidação `304` | funciona (novo em A2) |
| `CookieJar`: `Domain`/`Path`/`Expires`/`Max-Age`/`Secure`/`HttpOnly`, host-only | funciona (novo em A2) |
| Charset pelo `Content-Type` (`encoding_rs`), fallback UTF-8 → windows-1252 | funciona (novo em A2) |
| `POST application/x-www-form-urlencoded` por formulário | funciona (novo em A3) |
| Política de origem: CORS em `fetch`/XHR (`*` e origem exata) | funciona (novo em A7) |
| `file://` não alcança a rede; `https` não carrega `http` (mixed content) | funciona (novo em A7) |
| CSP: `script-src`/`style-src`/`img-src` com `'self'`, `'none'`, hosts | funciona (novo em A7) |
| Cache HTTP em disco, HTTP/2, redirecionamentos de método | não implementado (A8) |

### Interação

| Capacidade | Estado |
|---|---|
| Foco, digitação, `Tab`/`Shift+Tab`, `Enter` envia formulário | funciona |
| Clique em checkbox/radio/label/select/submit | funciona (novo em A3) |
| Popup de `<select>` com hover, `Escape` fecha | funciona (novo em A3) |
| Seleção de texto por arrasto, realce, `Ctrl+C` (`arboard`) | funciona (novo em A3) |
| Abas, barra de endereço, voltar/avançar/recarregar, janela sem moldura | funciona |

## Limitações conhecidas

1. **`fetch` e `XHR` são síncronos.** Uma requisição lenta congela a thread JS por
   até o timeout do transporte (15 s). Escolha deliberada para manter uma única
   thread de script e uma única ordem de execução; a alternativa (fila em thread com
   drenagem por timer) está registrada no ledger e não foi implementada.
2. **`structuredClone` é `JSON.parse(JSON.stringify(x))`.** Perde `Date`, `Map`,
   `Set`, funções e ciclos.
3. **`getSelection()` não expõe `Range`.** `toString()` e `isCollapsed` funcionam;
   `anchorNode`/`getRangeAt` devolvem `null`.
4. **`matchMedia` é um snapshot.** Não reavalia em resize, e só entende
   `min/max-width`/`height`, `all`, `screen` e `and`.
5. **`history` não restaura estado ao voltar.** `popstate` dispara e o índice muda,
   mas a página não é refeita a partir do estado salvo.
6. **CSP ignora nonces, hashes e `'unsafe-inline'`.** Um `script-src` sem host
   bloqueia todo script externo; `'unsafe-inline'` é aceito como fonte mas não
   distingue script inline de externo.
7. **`file://` é origem opaca.** Não alcança a rede e não é same-origin com nada,
   inclusive consigo mesma.
8. **Fontes web, `iframe`, `canvas`, gradientes, `transform` e `float` não existem**
   — são as fases A4, A5 e A6, especificadas e não executadas.
9. **Sem isolamento de processo.** Todo conteúdo remoto roda no mesmo processo do
   chrome; adiado explicitamente pelo roadmap.

## Números de `--bench` antes e depois

`--bench <alvo> 10`, mediana de 3 execuções, ms. `load` inclui a rede e varia com a
latência; as demais colunas são determinísticas.

| Alvo | parse | css | style | layout | alocs |
|---|---|---|---|---|---|
| `naro://start` | 0.012 → 0.015 | 0.017 → 0.023 | 0.063 → 0.077 | 0.044 → 0.058 | 16351 → 17290 |
| `https://example.com` | 0.008 → 0.006 | 0.008 → 0.005 | 0.041 → 0.036 | 0.018 → 0.013 | 7319 → 8032 |
| `https://www.rust-lang.org` | 0.509 → 0.481 | 3.156 → 3.075 | 2.878 → 2.199 | 1.175 → 1.022 | 413109 → 422183 |
| `https://developer.mozilla.org/en-US/docs/Web/HTML` | 1.829 → 1.543 | 1.806 → 1.712 | 12.869 → 11.828 | 5.912 → 5.089 | 733343 → 753481 |
| `https://www.google.com` | 0.844 → 0.811 | 3.950 → 3.264 | 2.343 → 2.921 | 0.481 → 0.618 | 432742 → 520701 |

Alocações sobem 3–20%: cada sessão agora carrega o vetor de estilos computados e a
tabela de geometria que o `getComputedStyle` e o `fetch` exigem. O tempo de estilo e
layout caiu ou ficou estável, porque nada disso roda no caminho quente.

## Páginas de referência (`--dump`, 1000 px)

| Página | STATUS | CSS | CONTENT_HEIGHT | JS ERROR |
|---|---|---|---|---|
| `https://example.com` | Idle | rules=4 invalid=0 | 820.60 | 0 |
| `https://www.rust-lang.org` | Idle | rules=2052 invalid=0 | 3968.98 | 0 |
| `https://developer.mozilla.org/en-US/docs/Web/HTML` | Idle | rules=427 invalid=10 | 16897.00 | 6 |
| `https://www.google.com` | Idle | rules=1276 invalid=0 | 487.90 | 8 |

Idênticas ao baseline de A0 em todas as quatro. Os 6 erros de MDN e os 8 de Google
são os mesmos de antes do marco (`export` em módulo ES, `not a function` em bundle
minificado), não regressões.

## Como verificar

```sh
export PATH="<mingw64>/bin:$PATH"
cargo +stable-x86_64-pc-windows-gnu test --workspace          # 235 passed
./target/release/naro.exe --compat tests/compat                # 30/0/3
./target/release/naro.exe --compat tests/compat --serve        # 32/0/1
./target/release/naro.exe --dump tests/compat/32-selection.html 1000 --select 1 25 999 60
./target/release/naro.exe --render tests/compat/32-selection.html out.bmp 1000 --select 1 25 999 60
```

## Fases seguintes (especificadas, não executadas)

A4 fontes web · A5a efeitos visuais · A5b transformações · A5c completude de layout ·
A6 `iframe` e `canvas` · A8 corpus e cache em disco · A9 desempenho.
Isolamento de processos para conteúdo remoto segue adiado.
