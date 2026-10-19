# Estado do roadmap

Marco em execução: **A5c** concluída (completude de layout). Plano:
`local://naro-roadmap-v03-plan.md`; roadmap global em `local://naro-global-roadmap-plan.md`.

## Fases

| Fase | Estado | Evidência | Tentativas | Notas |
|---|---|---|---|---|
| A0 | concluída | tag `baseline-f8` + `phase-a0`; `228 passed / 0 failed`; `--compat` 27/0/1; `computed:`/`attr:` verificados em positivo e negativo | 1 | baseline registrado abaixo |
| A1 | concluída | tag `phase-a1`; `228 passed / 0 failed`; `--compat` **28 pass / 0 falhas / 1 não suportado** (29 total); `29-platform.html` com 46 checks; 4 páginas de referência com `JS ERROR` idênticos ao baseline e `CONTENT_HEIGHT` idêntico; `diff reports/p8-rust.txt` = 0 | 1 | ver desvios abaixo |
| A2 | concluída | tag `phase-a2`; `233 passed / 0 failed`; `--compat` **28 pass / 0 falhas / 2 não suportados** (30) e `--compat --serve` **29 pass / 0 falhas / 1 não suportado**; `30-net.html` com 19 checks provando fetch, XHR, cache (1 requisição ao servidor para 3 pedidos), revalidação 304, cookie de ida e volta, `HttpOnly` invisível ao script, POST e charset windows-1252; 4 páginas de referência idênticas ao baseline; `diff reports/p8-rust.txt` = 0 | 1 | ver desvios abaixo |
| A3 | concluída | tag `phase-a3`; `233 passed / 0 failed`; `--compat` **30 pass / 0 falhas / 2 não suportados** (32) e `--compat --serve` **31 pass / 0 falhas / 1 não suportado**; `31-forms.html` (checkbox/radio/:checked/label/select/POST) e `32-selection.html` (`selection:` em duas direções); `--render ... --select` mostra 5090 px de realce nas linhas certas; 4 páginas de referência com `JS ERROR` e `CONTENT_HEIGHT` idênticos; `p8-rust` difere em 84 linhas, todas explicadas pela caixa do `<select>` | 1 | ver desvios abaixo |
| A7 | concluída | tag `phase-a7`; `235 passed / 0 failed`; `--compat` 30/0/3 e `--compat --serve` **32 pass / 0 falhas / 1 não suportado**; `33-origin.html` prova `same.ok=true`, `cross.star=cors-ok`, `cross.blocked=true` (TypeError, corpo nunca entregue) e `blocked:assets/red.png` (CSP `img-src 'self'`); `file://` não alcança a rede (`local.blocked=true`); 4 páginas de referência idênticas ao baseline; `diff reports/p8-rust-a3.txt` = 0 | 1 | ver desvios abaixo |
| Marco v0.2 | concluída | `reports/milestone-v0.2.md`; tag `milestone-v0.2` | 1 | execução aprovada encerrada |
| A4 | concluída | tag `phase-a4`; `251 passed / 0 failed`; `--compat` **30 / 0 / 4** (34) e `--compat --serve` **33 / 0 / 1**; `34-fonts.html` prova `font-family`, `font-weight`, `@font-face` de um TTF commitado e a síntese de negrito (`ff=monospace`, `fw=700`, `fx=Naro Fixture, sans-serif`, `runfont:`, `boxdiff:`); 4 páginas de referência com `JS ERROR` e `CONTENT_HEIGHT` idênticos; `p8-rust-v03` difere em 2 hunks (at-rule 45→0, baseline de 96 px em 0.7 px) | 1 | WOFF2 bloqueado, ver abaixo |
| A5a | concluída | tag `phase-a5a`; `268 passed / 0 failed`; `--compat` **31 / 0 / 4** (35) e `--compat --serve` **34 / 0 / 1**; `35-visual.html` com 8 `pixel:` (gradiente linear e radial, sombra com spread, círculo de raio percentual); `--bench-shadow 20` mede `(100,50)` em **6.98 ms** contra orçamento de 8 ms | 1 | ver desvios abaixo |
| A5b | concluída | tag `phase-a5b`; `290 passed / 0 failed`; `--compat` **32 / 0 / 4** (36) e `--compat --serve` **35 / 0 / 1**; `36-transform.html` com 8 `pixel:`, `box:` provando layout intocado e `hit:`/`nothit:` provando o clique na posição pintada; testes de matriz pai→filho, espelho e `scale(0)` | 1 | ver desvios abaixo |
| Marco v0.3 | concluída | `reports/milestone-v0.3.md`; tag `milestone-v0.3` | 1 | A6, A8 e A9 aguardam autorização |
| A5c | concluída | tag `phase-a5c`; `299 passed / 0 failed`; `--compat` **33 / 0 / 4** (37) e `--compat --serve` **36 / 0 / 1**; `37-layout-completo.html` com 9 `pixel:`, 3 `box:` e `computed:` para float/clear/order; `p8-rust-a5c` difere de `p8-rust-v03` em 2 linhas (baseline de dois rótulos "Language") | 1 | ver desvios abaixo |

## Próxima ação

**Diagnóstico estrutural de páginas reais** (hostinger, discord, chatgpt) antes de
A6, conforme a etapa 2 da autorização. A6 (iframe/canvas) só começa depois dele.

## Bloqueios

1. **`woff2` 0.3.0 não compila no rustc 1.99.1** — 23 erros, todos dentro do
   crate: `SafeBuf::try_get_u8` colide com `std::io::Buf::try_get_u8`
   (`E0034`, 15 ocorrências) e `?` não converte `Base128Error`/`Truncated`/
   `TableDirectoryError` (`E0277`). Tentativas: (a) adicionar `woff2 = "0.3"` e
   compilar — falhou como acima; (b) `cargo info woff2` para achar outra versão —
   0.3.0 é a única publicada; (c) remover a dependência. Decisão: **A4 segue com
   TTF/OTF apenas**; `register_web_family` devolve `Err("WOFF2 não suportado
   nesta build")` e `naro-net`/`prepare_fonts` pulam URLs `.woff2`/`.woff` para
   não gastar o orçamento de download. O fixture de `34-fonts.html` é um TTF OFL
   (`tests/compat/assets/fixture.ttf`, Roboto de `cdn.jsdelivr.net/fontsource`),
   porque commitar uma fonte do sistema é proibido por licença. Item "WOFF2"
   fica marcado como bloqueado, não como cumprido.

## Desvios e decisões de A5c

1. **`float`/`clear` com banda de exclusão real.** `Flow` mantém uma lista de
   `FloatRect` por block formatting context (salva e restaurada em cada `flow`).
   `LineBuilder` recebe a mesma lista e decide a quebra contra
   `band_width(y)`, e o `flush` desloca a linha inteira pelo offset esquerdo da
   banda — sem isso o texto era quebrado na largura certa mas desenhado em
   `x = 0`, por baixo do flutuante.
2. **O container cresce até o fundo do flutuante.** Navegadores deixam o pai em
   altura zero e dependem de clearfix; aqui o `flow` devolve
   `max(linha, cursor, fundo do float)`. Sem isso, duas seções de uma página real
   se sobrepunham — o defeito estrutural mais visível.
3. **`vertical-align` é aplicado no `flush`,** quando a baseline é conhecida: o
   átomo é disposto no topo da linha e deslocado por `(dx, dy)`. Imagens passaram
   a registrar `AtomMark` também, então `vertical-align` vale para `<img>`, não só
   para `inline-block`. Um átomo alinhado pela baseline também aumenta o ascent da
   linha, senão uma imagem alta era empurrada para fora dela.
4. **`border-collapse: collapse` divide a borda entre células vizinhas** (metade
   cada, mínimo 1 px), e a borda efetiva é a que a célula pinta. As posições das
   células não mudam — só a espessura desenhada.
5. **`order` ordena os filhos de flex e grid** com `sort_by_key` estável, então
   empates mantêm a ordem do documento. Não se aplica a fluxo de bloco normal.
6. **`grid-area` expande para `grid-row`/`grid-column`** no parser, incluindo a
   forma de quatro valores; `auto` em qualquer slot significa auto-colocação.
7. **Dois defeitos pré-existentes corrigidos junto**, porque são a mesma classe
   de erro que a etapa 2 investiga e ambos apareciam em página real:
   - `display: flex`/`grid` ignorava `height` declarado e colapsava para a altura
     do conteúdo (um cabeçalho `height: 100vh` virava 0 px);
   - um grid com altura definida não distribuía a sobra entre as linhas
     (`align-content: stretch`, o padrão), então os itens ficavam amontoados no
     topo e um `grid-area` de duas linhas media 0 px.
8. **`p8-rust` re-baselinado** em `reports/p8-rust-a5c.txt`: 2 linhas diferem de
   `p8-rust-v03` — o rótulo "Language" de um `<select>` desceu 8.6 px porque o
   átomo baseline agora aumenta o ascent da linha. As 144 caixas continuam
   idênticas e `CONTENT_HEIGHT` não mudou.

## Desvios e decisões de A5b

1. **Uma matriz por nó, e a mesma para pintar e clicar.** `Layout::transforms`
   guarda a matriz de página já acumulada com os ancestrais. A pintura faz
   `push_transform`; o hit-testing faz `inverse()` da **mesma** entrada. Para
   runs, imagens e links — que são keyed pelo nó que os produziu, muitas vezes um
   nó de texto — o engine propaga a matriz pela árvore uma vez
   (`effective_transforms`), o que dá a cada nó exatamente a matriz que o pintor
   usa, sem uma segunda fonte de verdade e sem 24 bytes por run.
2. **A caixa é medida uma vez como sonda.** `transform-origin` em `%` precisa da
   caixa em px, e a caixa só existe depois do layout. Um nó com `transform` é
   disposto uma vez, o resultado é descartado (marcadores de tamanho dos vetores
   de saída), a matriz é calculada e o nó é disposto de novo com a matriz na
   pilha. Layout não depende de transformação, então a segunda passada é
   idêntica; só nós transformados pagam.
3. **`geometry_differs` inclui `transform`, `transform_origin` e `object_fit`**
   apesar de não moverem caixas: a matriz e o sub-retângulo vivem no `Layout`,
   então um restyle que só muda `transform` precisa reconstruir o layout, senão a
   página continua pintando a matriz antiga. Há teste para isso
   (`restyling_transform_rebuilds_the_matrix`).
4. **`LinkBox` ganhou `node`** para que `hit_link` aplique a matriz do `<a>`. Sem
   isso um link deslocado seria clicável na posição de layout.
5. **Texto sob translação usa o caminho direto.** `draw_text` desloca a pena
   quando a matriz é translação com escala uniforme positiva — o glifo mantém o
   antialiasing e o custo — e só mapeia pixel a pixel em rotação, cisalhamento
   ou espelho. A prova visual do `--render` de `36-transform.html` mostrou o
   `<a>` deslocado com o texto parado na posição de layout; há teste de
   regressão (`translated_text_paints_at_the_translated_position`).
6. **Ordem de pintura do `body`**: um filho absoluto é pintado antes da caixa do
   ancestral (comportamento pré-existente), então o fixture usa `body`
   transparente. Registrado como limitação, não como mudança de critério.

## Desvios e decisões de A5a

1. **O teto de sombra é sobre a alocação total, e a máscara é recortada pelo
   clip.** `shadow_mask_bytes` conta os **dois** buffers (`u8`, máscara +
   temporário do blur horizontal); `clamp_shadow` reduz `blur`/`spread` por
   bisseção até caber em `MAX_SHADOW_PIXELS = 4_000_000`. Além disso a máscara é
   limitada ao clip, então uma caixa 5000×5000 num canvas 400×300 aloca a área
   visível, não 50 MB — é o que o teste `huge_shadow_stays_within_budget`
   exercita.
2. **`blur` é o suporte total do blur, não o raio por passada.** Três passadas
   com `radius = blur / 3` somam exatamente `blur` de cauda; usar `blur * 3` de
   padding (como no plano) reservava o dobro da área e custava 190 ms em
   `(100,50)`.
3. **Custo do blur**: soma deslizante nas duas passadas (O(1) por pixel
   independente do raio) e a passada vertical em ordem de linha, com somas por
   coluna. A versão ingênua somava a janela por pixel e levava 192 ms.
4. **O clamp não atua em `(100,50)` no `--bench-shadow`.** O plano previa que
   `efetivo` fosse menor que `100,50`; com o padding correto (item 2) e a máscara
   recortada, 300×300 + 2×(50+100) = 600×600 → 720 KB, dentro do teto de 4 MB.
   O clamp é exercitado onde importa: `clamp_shadow_respects_the_total_allocation`
   (blur 5000) e `huge_shadow_stays_within_budget` (caixa 5000×5000). O critério
   do gate — alocação limitada e ≤ 8 ms — é cumprido; só a expectativa numérica
   do exemplo do plano não se aplica.
5. **`box-shadow` de caixa quadrada mantém cantos quadrados**: `spread` cresce a
   forma, não a arredonda.
6. **`border-radius: %` é resolvido em `Flow::block`** (e repetido em
   `refresh_paint`, senão um restyle só de cor voltaria a quadrar a caixa).

## Desvios e decisões de A4

1. **`FontFace` substituiu `bold` na trait** e `TextRun` guarda
   `family`/`weight`/`italic`. A família é internada com `Arc<str>` por linha:
   um `String` por run custava uma alocação por run (medido: +0.4 ms de layout em
   `rust-lang.org`, reduzido com a interning).
2. **Resolução memoizada.** `FontSet` memoriza a última requisição (família,
   peso, inclinação) → índice de família + flags de síntese, e a lista
   `font-family` é dividida sem alocar. `font-family` desconhecida cai em
   `sans-serif`; `serif`/`monospace` são famílias reais registradas de arquivos do
   sistema, com fallback para o regular do sistema se o arquivo não existir.
3. **Orçamento de fontes por documento, não por passada.** `prepare_fonts` roda
   duas vezes (antes do layout provisório e depois dos scripts) e compartilha
   `font_sync_downloads` (teto 2), `font_cache` e `registered_faces`. Falha de
   download/decodificação é terminal para o documento: a URL fica `None` e nunca
   é retentada, então um `@font-face` quebrado não gera uma tentativa por
   restyle. O erro "limite de downloads" é emitido **uma vez** por documento.
4. **Erros de fonte são adiados.** A primeira passada roda antes de a sessão JS
   existir, e `self.js` é substituído ao iniciar a sessão; por isso os erros vão
   para `pending_font_errors` e são anexados quando o documento termina.
5. **`@font-face` deixou de contar como at-rule não suportada** — é a causa do
   `at-rule` cair a zero no `p8` de rust-lang e de 116→103 no MDN.
6. **`font:` shorthand** com peso numérico (`font: 700 24px Arial`) passou a
   tratar `700` como peso; antes o token numérico era lido como tamanho.

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

Estado após o marco v0.3 (`at-rule` cai porque `@font-face` virou suporte):

| Página | TITLE | STATUS | CSS | CONTENT_HEIGHT | JS ERROR |
|---|---|---|---|---|---|
| `https://example.com` | Example Domain | Idle | rules=4 invalid=0 at-rule=0 | 820.60 | 0 |
| `https://www.rust-lang.org` | Rust Programming Language | Idle | rules=2052 invalid=0 at-rule=0 | 3968.98 | 0 |
| `https://developer.mozilla.org/en-US/docs/Web/HTML` | HTML: HyperText Markup Language \| MDN | Idle | rules=427 invalid=10 at-rule=103 | 16897.00 | 6 |
| `https://www.google.com` | Google | Idle | rules≈1277 invalid=0 at-rule=35 | 487.90 | 8 |

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
