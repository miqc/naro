# Sprint 18 — Renderização real do Google

Captura final: `reports/google_s18.png` (comparar com `google_s17.png`).

## Causas confirmadas (com evidências)

| # | Sintoma | Evidência | Causa |
|---|---|---|---|
| 1 | Logo à esquerda | `IMG rect=(0.0,60.0,272.0,92.0)` antes | O Google usa `<center>`; o NARO não tinha regra de UA para `center` (`display:block; text-align:center`), então o elemento virava inline sem alinhamento |
| 2 | Identificador longo visível (`ABILxe8…`) | `<input value="ABIL…" name="iflsig" type="hidden">` | `type=hidden` não era tratado: o controle era pintado como campo de texto |
| 3 | Rodapé no meio do conteúdo | `div hidden` não suprimido | `[hidden]` e o atributo `hidden` não eram respeitados |
| 4 | Campo de pesquisa/ícone deslocados | `input rect=(0.0,199.2,458.0,32.0)` | Controles inline não aplicavam `text-align` do contêiner ao iniciar uma linha |
| 5 | `self is not defined` | 2 scripts abortavam | O bootstrap definia `window`, mas não `self`/`top`/`parent`/`frames` |

Nenhum desses sintomas vem de Shadow DOM. O Shadow DOM existe na página e continua ausente, mas os cinco defeitos acima são todos de DOM comum.

## Correções implementadas

| # | Correção | Arquivo |
|---|---|---|
| 1 | `center { display:block; text-align:center }` no UA stylesheet | `naro-css` |
| 2 | `is_hidden_node()`: ignora `[hidden]` e `input[type=hidden]` na construção da árvore de caixas | `naro-layout` |
| 3 | Controles inline centralizados/alinhados à direita quando iniciam a linha | `naro-layout` |
| 4 | `self`, `top`, `parent`, `frames` apontam para o mesmo objeto global de `window`/`globalThis` | `naro-js` |
| 5 | Erros JS incluem a primeira linha do stack: `not a function @ at <anonymous> (eval_script:78:28)` | `naro-js` |

## Testes

- `cargo test --workspace`: **58 passando, 0 falhas**.
- `naro --compat tests/compat`: **16 PASS, 0 FAIL, 1 UNSUP (total 17)**.
  Novo caso `17-center.html` cobre `<center>`, `[hidden]` e `input[type=hidden]`.

## Capturas antes/depois

| Aspecto | `google_s17.png` | `google_s18.png` |
|---|---|---|
| Logo | x=0, colado à esquerda | **x=364, centralizado** |
| Botões | x=7, alinhados à esquerda | **x=460/462, centralizados** |
| `ABILxe8…` | visível na página | **ausente** |
| "Pesquisa avançada" | x=13 | x=470, centralizado |
| Rodapé | sobreposto ao conteúdo | abaixo do conteúdo, centralizado |

`CONTENT_HEIGHT` do Google: 507 → 620 px (o conteúdo deixou de colapsar).

## Erros JavaScript restantes

```
<script> #2: not a function @ at <anonymous> (eval_script:78:28)
<script> #8: not a function @ at <anonymous> (eval_script:11:28)
<script> #9: not a function @ at <anonymous> (eval_script:40:28)
```

`self is not defined` desapareceu. Os três restantes são `TypeError` de APIs
ausentes (`requestIdleCallback`, `PerformanceObserver`, `Image`). Não foram
corrigidos nesta sprint porque o layout já não depende deles para os sintomas
investigados.

## O que continua errado no Google

- **Campo de pesquisa invisível**: o `<input>` tem 458×32 e borda, mas o
  preenchimento visual (fundo, sombra, placeholder) vem de `border-radius`,
  `box-shadow` e do placeholder via JS — nenhum suportado.
- **Ícone de teclado flutuando** em x=458: é um `<img>` dentro do wrapper do
  campo; sem `position:absolute` correto ele participa do fluxo inline.
- **Faixa de topo sem fundo**: `background` do header depende de `position:
  sticky` e de custom properties em `@media`.
- **Sem três blocos verticais**: `display: contents` e Shadow DOM ausentes.

## Disco

12 GB livres antes e depois. Dois builds de release; nenhum diretório removido.
