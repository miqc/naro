# Marco v0.3 — A4 (fontes web), A5a (efeitos visuais), A5b (transformações)

Data: 2026-10-18. Toolchain `+stable-x86_64-pc-windows-gnu` (rustc 1.99.1), PATH
do MinGW UCRT com separador `;`. Raiz `C:/Users/Mica/Documents/GitHub/naro`.

## Gates

| Fase | `cargo test --workspace` | `--compat` | `--compat --serve` |
|---|---|---|---|
| v0.2 (partida) | 235 / 0 | 30 / 0 / 3 (33) | 32 / 0 / 1 (33) |
| A4 | 251 / 0 | **30 / 0 / 4** (34) | **33 / 0 / 1** (34) |
| A5a | 268 / 0 | **31 / 0 / 4** (35) | **34 / 0 / 1** (35) |
| A5b | 290 / 0 | **32 / 0 / 4** (36) | **35 / 0 / 1** (36) |

`14-form.html` continua o único `UNSUP` em `--serve`; em `file://` os não
suportados são `14-form`, `30-net`, `33-origin` e `34-fonts`, todos por exigirem
origem http real. Nenhum limiar existente mudou e nenhum teste foi removido.

### Prova de comportamento novo

- **A4** — `--compat --serve --verbose` imprime
  `LOG 34-fonts.html: ff=monospace`, `LOG 34-fonts.html: fw=700` e
  `LOG 34-fonts.html: fx=Naro Fixture, sans-serif`; as checks `runfont:monospace`,
  `runfont:naro fixture`, `boxdiff:#mono:#sans:1` e `boxdiff:#fix:#sans:1`
  passam. `boxdiff` prova que a família muda a medição sem fixar um número que
  dependeria da fonte instalada.
- **A5a** — `35-visual.html` verifica o rasterizador por `pixel:`:
  `(100,50)=#808080` (meio do gradiente preto→branco), `(10,50)=#0d0d0d`,
  `(400,50)=#ffffff` e `(305,5)=#000000` no radial, `(105,225)=#000000` /
  `(50,225)=#ffffff` na sombra de spread sólido, e `(302,202)=#ffffff` /
  `(350,250)=#ff0000` no círculo de `border-radius: 50%`.
- **A5b** — `36-transform.html`: `pixel:(250,50)=#ff0000` (destino novo) e
  `pixel:(50,50)=#ffffff` (posição de layout vazia) com `box:#t:100x100`
  provando que o layout não se moveu; `pixel:(20,210)=#0000ff` (centro girado),
  `pixel:(20,195)=#0000ff` (área girada) e `pixel:(5,210)=#ffffff`;
  `hit:430,310=#lk` e `nothit:30,310=#lk` provam que o `<a>` deslocado é
  clicável onde é pintado e não onde o layout o colocou.

### `--bench-shadow 20` (release, escala 1, caixa 300×300)

| blur | spread | efetivo | ms (mediana) |
|---|---|---|---|
| 0 | 0 | 0,0 | 0.251 |
| 10 | 0 | 10,0 | 2.126 |
| 50 | 25 | 50,25 | 4.243 |
| 100 | 50 | 100,50 | **6.980** |

Orçamento de ≤ 8 ms por chamada em `(100,50)`: **cumprido**, com medição real
registrada acima. O clamp **não** atuou nesse par — ver "Desvios", item 4.

## Matriz de capacidade

### Tipografia

| Recurso | Antes | Agora |
|---|---|---|
| `font-family` (lista, cascata, herança) | não modelado | resolvido por família registrada; genéricos `sans-serif`/`serif`/`monospace`/`system-ui`/`ui-*`; família desconhecida cai no padrão |
| `font-weight` numérico | só 400/700 efetivos | seleção do face mais próximo (exato → mesmo peso com outra inclinação → peso mais próximo) |
| `font-style: italic/oblique` | ignorado | face itálico real, ou cisalhamento sintético de 0.21 |
| Negrito sem arquivo bold | face bold do sistema | dilatação horizontal de 1 px + `0.03 em` de avanço |
| `@font-face` + `src: url()` | at-rule não suportada | regra parseada, `local()` descartado, URLs em ordem |
| Fontes web TTF/OTF | — | baixadas (prefetch de até 6, +2 síncronos por documento), registradas antes do layout |
| Fontes web WOFF2/WOFF | — | **não suportadas nesta build** (ver Bloqueios) |

### Pintura

| Recurso | Antes | Agora |
|---|---|---|
| `rgba()`/`#rrggbbaa`/`#rgba` | alfa ignorado | alfa real, composto em `blend_at` |
| `hsl()`/`hsla()` | não parseado | convertido para sRGB |
| `linear-gradient()` | não pintado | ângulo em graus e `to <lado>`, stops com posições |
| `radial-gradient()` | não pintado | centro e raio em fração da caixa |
| `box-shadow` | 3 retângulos empilhados, sem spread/inset | máscara borrada (3 passadas separáveis), `spread`, `inset`, caminho rápido sem alocação |
| `border-radius: %` | ignorado | resolvido contra a caixa em `Flow::block` |

### Transformação

| Recurso | Agora |
|---|---|
| `transform` | `translate(X/Y)`, `scale(X/Y)`, `rotate(deg/rad)`; composição `A·B·C` da esquerda para a direita |
| `transform-origin` | palavras-chave e `%`; `0`/`0px` exatos |
| Herança de matriz | filho acumula a matriz do pai (uma matriz por nó em `Layout::transforms`) |
| Pintura sob transformação | retângulo/arredondado por mapeamento inverso com supersampling só nas bordas; imagem bilinear; texto com deslocamento de pena quando a matriz é translação+escala, mapeamento pixel a pixel em rotação/cisalhamento |
| Hit-testing | inversa da **mesma** matriz da pintura, para bloco e para link |
| `scale(0)` | invisível e não clicável |
| `object-fit` | `fill`/`contain`/`cover`/`none`/`scale-down` |
| `aspect-ratio` | altura derivada da largura quando não há `height` |

### Imagem

| Recurso | Agora |
|---|---|
| `object-fit` no `<img>` | sub-retângulo de destino calculado na pintura |
| `aspect-ratio` no `<img>` | decide o eixo que falta antes da proporção intrínseca |
| imagem sob `transform` | amostragem bilinear por mapeamento inverso |

## Limitações

Novas nesta fase:

1. **WOFF2 e WOFF não decodificam** — `woff2` 0.3.0 não compila no rustc 1.99
   (ver Bloqueios). Páginas que só oferecem WOFF2 caem na família seguinte da
   lista. `naro-net` e `prepare_fonts` pulam essas URLs para não gastar o
   orçamento de download em bytes inutilizáveis.
2. **Gradiente ignora `border-radius`** — a caixa é preenchida retangular.
3. **Texto girado perde antialiasing de borda** — o glifo é mapeado pixel a
   pixel pelo afim, então um `rotate` produz contorno pontilhado a 1×. Texto só
   transladado ou escalado continua no caminho direto.
4. **`hit_text` e a seleção usam os runs não transformados** — selecionar texto
   girado escolhe a linha errada; texto transladado ou escalado acerta.
5. **`transform-origin` em px diferente de zero** cai no padrão (a fração exige a
   caixa, que o parser não tem); `%` e palavras-chave funcionam.
6. **`box-shadow` só pinta a primeira sombra de uma lista.**
7. **Ordem de pintura pré-existente**: um filho `position: absolute` é pintado
   antes da caixa do ancestral, então um `background` no `<body>` cobre os
   filhos absolutos. `36-transform.html` usa `body` transparente por isso; é
   limitação anterior a este marco, não uma regressão.

Herdadas, ainda válidas: sem `float`/`clear`, sem `vertical-align`,
`border-collapse` não colapsa, sem `order`, sem `grid-area`, sem `iframe`,
sem `<canvas>`, sem animações.

## Desempenho (`--bench`, mediana de 3)

| Alvo | layout v0.2 | layout v0.3 | paint v0.2 | paint v0.3 |
|---|---|---|---|---|
| `naro://start` | 0.044 | 0.058 | 0.518 | 0.551 |
| `https://example.com` | 0.018 | 0.024 | 0.292 | 0.374 |
| `https://www.rust-lang.org` | 0.90 | 1.36 | 0.78 | 0.85 |
| `https://developer.mozilla.org/en-US/docs/Web/HTML` | 5.91 | 6.31 | — | — |

O layout de `rust-lang.org` ficou ~0.4 ms mais lento: cada palavra agora resolve
família + peso + inclinação antes de medir. Duas mitigações já aplicadas — a
lista de famílias é dividida sem alocar e a última resolução é memorizada — e a
string de família é internada com `Arc<str>` por linha, então uma página com
milhares de runs aloca uma vez por família em vez de uma vez por run. O número
absoluto é pequeno diante de uma carga de ~20 ms; A9 (desempenho) é a fase que
trata disso com orçamento.

## Diff de `p8-rust`

`reports/p8-rust.txt` (v0.2) vs `reports/p8-rust-v03.txt` (este marco): **2 hunks**.

```
@@ -5 +5 @@
-CSS: ... at-rule=45 ...
+CSS: ... at-rule=0 ...
@@ -1169 +1169 @@
-  (12.0,401.5) "Rust" size=96.0 ...
+  (12.0,400.8) "Rust" size=96.0 ...
```

1. **`at-rule` 45 → 0**: `@font-face` deixou de ser uma at-rule não suportada —
   agora é parseada e registrada. As demais at-rules do site continuam contadas
   onde estavam; o número não é uma regressão, é a fase funcionando.
2. **Baseline do `<h1>` de 96 px, 0.7 px acima**: a linha passou a ter o ascent
   máximo entre as faces resolvidas por família em vez de sempre o regular do
   sistema. **Nenhuma caixa mudou**: as 144 linhas `rect=` são idênticas e
   `CONTENT_HEIGHT` continua 3968.98.

## Páginas de referência (`--dump`, 1000 px)

| Página | CONTENT_HEIGHT v0.2 | v0.3 | CSS `at-rule` v0.2 → v0.3 | JS ERROR |
|---|---|---|---|---|
| `example.com` | 820.60 | 820.60 | 0 → 0 | 0 → 0 |
| `www.rust-lang.org` | 3968.98 | 3968.98 | 45 → 0 | 0 → 0 |
| `developer.mozilla.org/…/HTML` | 16897.00 | 16897.00 | 116 → 103 | 6 → 6 (mesmas classes) |
| `www.google.com` | 487.90 | 487.90 | 37 → 35 | 8 → 8 (URLs mudam a cada carga) |

Nenhum `JS ERROR` novo. A queda de `at-rule` em MDN e Google é o mesmo efeito do
item 1 do diff: `@font-face` virou suporte. Google serve HTML diferente a cada
carga (já documentado), então `rules`/`pseudo-element` variam entre execuções.
