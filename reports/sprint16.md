# Sprint 16 — Diagnóstico de fidelidade visual (Google)

Capturas: `google_antes.png` e `google_depois.png`.

## Diagnóstico (evidências)

O Google carrega e o JavaScript roda parcialmente. Os erros observados no dump:

```
JS ERROR: <script> #2: not a function
JS ERROR: <script> #3: self is not defined
JS ERROR: <script> #8: not a function
JS ERROR: <script> #9: not a function
JS ERROR: <script> #11: self is not defined
```

Nenhum deles é a causa principal do layout. O que estava errado, medido no dump:

1. **Todo `<input>` ocupava 1000 px de largura e 10 px de altura.**
   `input rect=(0.0,195.2,1000.0,10.0)` — a regra de UA os tornava
   `display: block`, e um controle vazio não gerava linha de texto, então a
   altura era só borda+padding.
2. **Espaços entre elementos inline eram descartados.** `<a>Gmail</a>\n<a>Imagens</a>`
   colava os textos; o mesmo acontecia entre palavras separadas por elementos.
3. **Imagens inline ignoravam `text-align`.** O logo do Google (uma `<img>`
   centrada) ficava colado à esquerda.

O Shadow DOM existe na página, mas **não é a causa** dos três sintomas acima:
eles aparecem em nós do documento normal (barra de topo, formulário, logo).

## Correções implementadas

| # | Correção | Arquivo |
|---|---|---|
| 1 | `input`/`button` voltam a ser **inline-level**: `FlowItem::InlineBox`, largura shrink-to-fit (padrão de 160 px para `input`), texto/placeholder pintado dentro da caixa, cursor da linha avançado e reiniciado | `naro-css`, `naro-layout` |
| 2 | **Espaços inline preservados** (`InlineItem::Space`): separam palavras, não iniciam linha, não afetam colapso de margens | `naro-layout` |
| 3 | **Imagens inline respeitam `text-align`** do contêiner | `naro-layout` |

Além disso, controles vazios passaram a reservar uma linha (`font-size × line-height`),
o que corrigiu a altura de 10 px.

## Testes

- `cargo test --workspace`: **54 passando, 0 falhas**.
- `naro --compat tests/compat`: **14 PASS, 0 FAIL, 1 UNSUP (total 15)** — baseline mantido.
- `14-form.html` renderizado: rótulos, campos e botão alinhados à esquerda,
  sem deslocamento horizontal (antes o segundo campo aparecia em x=161).

## Resultado visual no Google

| Aspecto | Antes | Depois |
|---|---|---|
| Logo | colado à esquerda, pequeno | centralizado, 272 px de largura |
| Campos de busca | 1000×10 px, um por linha | 458×32 px, inline com o botão |
| Botões "Pesquisa Google" / "Estou com sorte" | não apareciam | visíveis, com texto |
| Links do topo (Gmail/Imagens) | colados | separados |
| Rodapé | sobreposto | em linha, com separadores |

O que **continua errado**: o campo de busca aparece como uma caixa branca vazia
(o Google preenche o `placeholder`/`aria-label` via JS, que falha em
`self is not defined`), e a página não tem o layout de três blocos porque
depende de `display: contents`, `position: fixed` e Shadow DOM.

## Limitações remanescentes

- `self`, `window.Image`, `fetch`, `requestAnimationFrame` ausentes — o JS do
  Google aborta cedo.
- `display: contents`, `position: fixed` (tratado como absolute), Shadow DOM,
  `@media` (descartado, não avaliado).
- Controles inline ocupam uma linha própria: dois `<input>` consecutivos não
  ficam lado a lado, diferente do navegador.

## Disco

12 GB livres antes e depois. Dois builds de release; nenhum diretório removido.
