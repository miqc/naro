# Estado do roadmap

Marco em execução: **v0.2** (fases A0, A1, A2, A3, A7). Plano: `local://naro-global-roadmap-plan.md`.

## Fases

| Fase | Estado | Evidência | Tentativas | Notas |
|---|---|---|---|---|
| A0 | concluída | tag `baseline-f8` + `phase-a0`; `228 passed / 0 failed`; `--compat` 27/0/1; `computed:`/`attr:` verificados em positivo e negativo | 1 | baseline registrado abaixo |
| A1 | concluída | tag `phase-a1`; `228 passed / 0 failed`; `--compat` **28 pass / 0 falhas / 1 não suportado** (29 total); `29-platform.html` com 46 checks; 4 páginas de referência com `JS ERROR` idênticos ao baseline e `CONTENT_HEIGHT` idêntico; `diff reports/p8-rust.txt` = 0 | 1 | ver desvios abaixo |
| A2 | concluída | tag `phase-a2`; `233 passed / 0 failed`; `--compat` **28 pass / 0 falhas / 2 não suportados** (30) e `--compat --serve` **29 pass / 0 falhas / 1 não suportado**; `30-net.html` com 19 checks provando fetch, XHR, cache (1 requisição ao servidor para 3 pedidos), revalidação 304, cookie de ida e volta, `HttpOnly` invisível ao script, POST e charset windows-1252; 4 páginas de referência idênticas ao baseline; `diff reports/p8-rust.txt` = 0 | 1 | ver desvios abaixo |
| A3 | concluída | tag `phase-a3`; `233 passed / 0 failed`; `--compat` **30 pass / 0 falhas / 2 não suportados** (32) e `--compat --serve` **31 pass / 0 falhas / 1 não suportado**; `31-forms.html` (checkbox/radio/:checked/label/select/POST) e `32-selection.html` (`selection:` em duas direções); `--render ... --select` mostra 5090 px de realce nas linhas certas; 4 páginas de referência com `JS ERROR` e `CONTENT_HEIGHT` idênticos; `p8-rust` difere em 84 linhas, todas explicadas pela caixa do `<select>` | 1 | ver desvios abaixo |
| A7 | concluída | tag `phase-a7`; `235 passed / 0 failed`; `--compat` 30/0/3 e `--compat --serve` **32 pass / 0 falhas / 1 não suportado**; `33-origin.html` prova `same.ok=true`, `cross.star=cors-ok`, `cross.blocked=true` (TypeError, corpo nunca entregue) e `blocked:assets/red.png` (CSP `img-src 'self'`); `file://` não alcança a rede (`local.blocked=true`); 4 páginas de referência idênticas ao baseline; `diff reports/p8-rust-a3.txt` = 0 | 1 | ver desvios abaixo |
| Marco v0.2 | concluída | `reports/milestone-v0.2.md`; tag `milestone-v0.2` | 1 | execução aprovada encerrada |

## Próxima ação

Nenhuma: o marco v0.2 está encerrado. A4, A5, A6, A8 e A9 estão especificadas e
aguardam autorização.

## Bloqueios

Nenhum.

## Desvios e decisões de A7

1. A política é aplicada **no ponto onde os bytes param**, não depois: `may_load`
   (mixed content e `file://` → rede) e `Csp::allows` rodam em
   `inline_stylesheets`/`fetch_images`/`fetch_scripts`, antes de qualquer requisição.
   Um recurso barrado entra em `FetchedPage::blocked` e o engine o publica como
   `recurso bloqueado pela política de origem: <url>` — é o que a check `blocked:`
   verifica.
2. CORS é decidido em `cors_verdict(page_origin, response)` dentro da host fn
   `__naro_fetch`, que é o caminho de `fetch` **e** de `XMLHttpRequest`. Um bloqueio
   devolve status 0 e o bootstrap transforma isso em `Promise.reject(TypeError)`, que
   é a forma que uma página espera.
3. Origem local (`file:`, `naro:`, `data:`, `about:`) é **opaca**: nunca é
   same-origin com nada, inclusive consigo mesma, e nunca é admitida por `*`.
4. CSP lê o header e `<meta http-equiv>` (o header vence). Nonces, hashes e
   `'unsafe-inline'` são aceitos como fonte mas não distinguem script inline de
   externo — limitação registrada no relatório do marco.
5. `HttpResponse` passou a carregar os headers que uma página pode observar
   (`content-type`, `cache-control`, `etag`, `last-modified`, `access-control-*`,
   `content-security-policy`, `location`). `Set-Cookie` fica de fora de propósito.
6. Defeito encontrado e corrigido no servidor de `--serve`: o corpo era convertido
   para `String` em todas as respostas, o que corrompia PNG e derrubava
   `04-images.html`/`17-center.html` em modo `--serve`. Só HTML passa por conversão
   (para substituir `__PORT__`).

## Desvios e decisões de A3

1. **`p8-rust.txt` deixou de bater: 84 linhas de diferença, todas por causa do
   `<select>`.** Antes de A3 o `<select>` não gerava caixa (era um elemento inline
   vazio), então `div#select` ocupava 812 px; agora o `<select>` é um controle
   inline-block com caixa própria (174x23.2) e o `div#select` shrink-wrappa (246 px).
   O deslocamento vertical de 0,4 px que isso causa empurra o rodapé inteiro. As duas
   únicas linhas novas são `select#`, e nenhuma caixa some. Nova referência guardada em
   `reports/p8-rust-a3.txt`; `reports/p8-rust.txt` fica intacto como baseline histórico.
2. `:checked` **passou a casar de verdade** (`Pseudo::Checked`): `input[checked]` e
   `option[selected]`. Antes caía em `Pseudo::Interactive`, que nunca casa. Mudança
   deliberada, coberta por `31-forms.html` (`computed:`/`attr:`).
3. A regra de estado ficou **uma vez só**, em `Document::toggle_checked`
   (`naro-html`). O engine aplica na árvore viva quando há sessão e no próprio snapshot
   quando não há, o que é o que impede `el.checked` e o atributo de divergirem.
4. `element.click()` de script roda a ação padrão **na hora** para um toggle (para que
   `el.checked` já esteja certo na instrução seguinte) e enfileira o resto (submit,
   reset, popup) para o `flush`. O engine drena a fila em `flush` e no fim da carga,
   antes de estilizar, e devolve a árvore resultante à sessão (`sync_session`).
5. `matchMedia`, `URL`, `history` e o resto de A1 continuam como em A1.
6. O popup do `<select>` é chrome (`App::select_popup`/`draw_select_popup`): desenhado
   depois da página, hit-testado antes dela, fecha com `Escape`. Num run headless ele
   não existe, por isso a check `choose:` chama `Engine::open_select`/`choose_option`,
   que é o mesmo caminho que o popup usa.
7. Cópia com `Ctrl+C` usa `arboard`; sem sessão gráfica a cópia é ignorada, e a prova
   determinística da seleção é `--select`/`--render`/`selection:`.

## Desvios e decisões de A2

1. A sessão de rede é **de processo** (`naro_net::session()`, `LazyLock`), não um
   campo `App { net: Arc<Mutex<NetSession>> }` como o plano descrevia. Efeito prático
   igual (keep-alive, cache e cookies sobrevivem à navegação), com dois ganhos: os
   threads de fetch não precisam receber o `Arc`, e `--compat`, `--bench` e o modo
   headless usam a mesma sessão sem plumbing. Concorrência preservada: só o cache e o
   jar de cookies ficam atrás de `Mutex`, nunca a requisição.
2. `fetch`/`XHR` bloqueiam a thread JS, como planejado, mas descobriu-se que as
   reações de `Promise` **não rodavam de jeito nenhum**: `drain_microtasks` só drenava
   a fila do `queueMicrotask` do bootstrap, e o QuickJS guarda as reações de promessa
   nativa na própria job queue. Agora `drain_microtasks` também chama
   `Ctx::execute_pending_job` (limite de 1000 jobs, com erro se estourar). Sem isso
   nenhuma página real com `fetch(...).then(...)` funcionaria.
3. `--compat` ganhou `--verbose` (imprime `LOG`/`ERR` de cada página) e a chave de
   manifesto `served`, que conta o caso como não suportado em `file://`. Sem isso a
   página de rede não teria como passar nos dois modos.
4. `NetSession::request` devolve status não-2xx como resposta, não como erro: um
   `fetch` precisa ver 404. Só falha de transporte vira `Err`.
5. Cookies: `HttpOnly` é enviado no fio e escondido de `document.cookie`; cookie sem
   `Domain` é *host-only* (não vai para subdomínio), e `Max-Age=0`/`Expires` no
   passado remove.
6. Charset: `encoding_rs` decodifica pelo `charset=` declarado; sem declaração,
   UTF-8 estrito e depois `windows-1252`.

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
