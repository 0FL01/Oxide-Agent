// Consent-O-Matic engine — stripped from browser extension.
// Source: https://github.com/cavi-au/Consent-O-Matic (MIT)
// Stripped: ES module imports/exports removed, chrome.* APIs replaced.
// No-op overrides for UI methods are applied by the Rust bootstrap.


class Tools {
    static setBase(base) {
        Tools.base = base;
    }

    static getBase() {
        return Tools.base;
    }

    static findElement(options, parent = null, multiple = false) {
        let possibleTargets = null;

        if(options.selector.trim() === ":scope") {
            //Select current root
            if(parent != null) {
                possibleTargets = [parent];
            } else {
                if (Tools.base != null) {
                    possibleTargets = [Tools.base];
                } else {
                    possibleTargets = [document];
                }

            }

            if(ConsentEngine.debugValues.debugRules) {
                console.log("Special :scope handling, selecting current root:", possibleTargets);
            }
        } else {
    	    // Find the right point to start from
            let top = document;
            if (parent != null) {
                top = parent;
            } else if (Tools.base != null){
                top = Tools.base;
            }

    	    // Consider shadow roots, even closed ones
            if (top.shadowRoot){
                top = top.shadowRoot;
            } else if (top.openOrClosedShadowRoot){ // Firefox
    	        top = top.openOrClosedShadowRoot;
            }
            possibleTargets = Array.from(top.querySelectorAll(options.selector));
        }

        const clonedPossibleTargets = possibleTargets.slice();

        if (options.textFilter != null) {
            let filterMultipleSpacesRegex = /\s{2,}/gm;

            possibleTargets = possibleTargets.filter((possibleTarget) => {
                let textContent = possibleTarget.textContent.toLowerCase().replace(filterMultipleSpacesRegex, " ");

                if (Array.isArray(options.textFilter)) {
                    let foundText = false;

                    for (let text of options.textFilter) {
                        if (textContent.indexOf(text.toLowerCase().replace(filterMultipleSpacesRegex, " ")) !== -1) {
                            foundText = true;
                            break;
                        }
                    }

                    return foundText;
                } else if (options.textFilter != null) {
                    return textContent.indexOf(options.textFilter.toLowerCase()) !== -1;
                }
            });
        }

        if (options.styleFilters != null) {
            possibleTargets = possibleTargets.filter((possibleTarget) => {
                let styles = window.getComputedStyle(possibleTarget);

                let keep = true;

                for (let styleFilter of options.styleFilters) {
                    let option = styles[styleFilter.option]

                    if (styleFilter.negated) {
                        keep = keep && (option !== styleFilter.value);
                    } else {
                        keep = keep && (option === styleFilter.value);
                    }
                }

                return keep;
            });
        }

        if (options.displayFilter != null) {
            possibleTargets = possibleTargets.filter((possibleTarget) => {
                if(possibleTarget.matches(".ConsentOMatic-CMP-NoDetect")) {
                    return !options.displayFilter;
                }

                if(options.displayFilter) {
                    //We should be displayed
                    return possibleTarget.offsetHeight !== 0;
                } else {
                    //We should not be displayed
                    return possibleTarget.offsetHeight === 0;
                }
            });
        }

        if (options.iframeFilter != null) {
            possibleTargets = possibleTargets.filter((possibleTarget) => {
                if(options.iframeFilter) {
                    //We should be inside an iframe
                    return window.location !== window.parent.location;
                } else {
                    //We should not be inside an iframe
                    return window.location === window.parent.location;
                }
            });
        }

        if(options.childFilter != null) {
            possibleTargets = possibleTargets.filter((possibleTarget) => {
                let oldBase = Tools.base;
                Tools.setBase(possibleTarget);
                let childResults = Tools.find(options.childFilter);
                Tools.setBase(oldBase);
                if(options.childFilterNegated) {
                    return childResults.target == null;
                } else {
                    return childResults.target != null;
                }
            });
        }

        if(ConsentEngine.debugValues.debugRules) {
            console.groupCollapsed("findElement:", options.selector, possibleTargets.length);
            console.log("Options:", options, "Parent:", parent);
            console.log("Possible targets before filter: ", clonedPossibleTargets);
            console.log("Possible targets after filter: ", possibleTargets);
            console.log("Returned result:", multiple?possibleTargets:possibleTargets[0]);
            console.groupEnd();
        }

        if (multiple) {
            return possibleTargets;
        } else {
            if (possibleTargets.length > 1) {
                if(ConsentEngine.debugValues.debugLog) {
                    console.warn("Multiple possible targets: ", possibleTargets, options, parent);
                }
            }

            return possibleTargets[0];
        }
    }

    static find(options, multiple = false) {
        let results = [];
        if (options.parent != null) {
            let parent = Tools.findElement(options.parent, null, multiple);
            if (parent != null) {
                if (parent instanceof Array) {
                    parent.forEach((p) => {
                        let targets = Tools.findElement(options.target, p, multiple);
                        if (targets instanceof Array) {
                            targets.forEach((target) => {
                                results.push({
                                    "parent": p,
                                    "target": target
                                });
                            });
                        } else {
                            results.push({
                                "parent": p,
                                "target": targets
                            });
                        }
                    });

                    return results;
                } else {
                    let targets = Tools.findElement(options.target, parent, multiple);
                    if (targets instanceof Array) {
                        targets.forEach((target) => {
                            results.push({
                                "parent": parent,
                                "target": target
                            });
                        });
                    } else {
                        results.push({
                            "parent": parent,
                            "target": targets
                        });
                    }
                }
            }
        } else {
            let targets = Tools.findElement(options.target, null, multiple);
            if (targets instanceof Array) {
                targets.forEach((target) => {
                    results.push({
                        "parent": null,
                        "target": target
                    });
                });
            } else {
                results.push({
                    "parent": null,
                    "target": targets
                });
            }
        }

        if (results.length === 0) {
            results.push({
                "parent": null,
                "target": null
            });
        }

        if (multiple) {
            return results;
        } else {
            if (results.length !== 1) {
                console.warn("Multiple results found, even though multiple false", results);
            }

            return results[0];
        }
    }
}

Tools.base = null;

class Matcher {
    static createMatcher(config) {
        switch(config.type) {
            case "css": return new CssMatcher(config);
            case "checkbox": return new CheckboxMatcher(config);
            case "url": return new URLMatcher(config);
            case "onoff": return new OnOffMatcher(config);
            default: {
                throw new Error("Unknown matcher type: "+config.type);
            }
        }
    }

    constructor(config) {
        this.config = config;
    }

    matches() {
        console.log("Remember to override matches()");
    }

    async debug(shouldMatch) {
        let result = Tools.find(this.config);

        let blinker = result.parent || result.target;

        if(blinker != null) {
            if(blinker.matches("input")) {
                blinker = blinker.parentNode;
            }

            let matches = this.matches();
            let correct = shouldMatch === matches;

            if (ConsentEngine.debugValues.clickDelay) {
                blinker.scrollIntoView({
                    behavior: "smooth",
                    block: "center",
                    inline: "center"
                });
            }

            if(correct) {
                blinker.style.border = "2px solid lime";
                blinker.style.backgroundColor = "lime";
            } else {
                blinker.style.border = "2px solid pink";
                blinker.style.backgroundColor = "pink";
            }

            await new Promise((resolve, reject)=>{
                if (ConsentEngine.debugValues.clickDelay) {
                        setTimeout(()=>{
                        resolve();
                    }, 10);
                } else {
                    resolve();
                }
            });
        }
    }
}

class OnOffMatcher extends Matcher {
    constructor(config) {
        super(config);
    }

    matches() {
        if(this.config.onMatcher == null || this.config.offMatcher == null) {
            throw new Error("Missing onMatcher/offMatcher on OnOffMatcher: "+JSON.stringify(this.config));
        }

        let onResult = Tools.find(this.config.onMatcher);
        let offResult = Tools.find(this.config.offMatcher);

        if(onResult.target == null && offResult.target == null) {
            throw new Error("Did not find neither on or off targets: "+JSON.stringify(this.config));
        }

        if(onResult.target != null && offResult.target != null) {
            throw new Error("Found both on and off targets: "+JSON.stringify(this.config));
        }

        return onResult.target != null;
    }

    async debug(shouldMatch) {
        let onResult = Tools.find(this.config.onMatcher);
        let offResult = Tools.find(this.config.offMatcher);

        let blinker = [];

        if(onResult.target == null && offResult.target == null) {
            //Neither On nor Off was found, this is wrong, but we have nothing to highlight.
            return;
        }

        if(onResult.target != null && offResult.target != null) {
            //Both On and Off was found, this is wrong!
            blinker.push({target: onResult.target, currect: false});
            blinker.push({target: offResult.target, currect: false});
        } else {
            //Since not both was null, and not both was not null, excactly 1 is not null

            if(shouldMatch) {
                if(onResult.target != null) {
                    blinker.push({target: onResult.target, correct: true});
                } else {
                    blinker.push({target: offResult.target, correct: false});
                }
            } else {
                if(onResult.target != null) {
                    blinker.push({target: onResult.target, correct: false});
                } else {
                    blinker.push({target: offResult.target, correct: true});
                }
            }
        }

        for(let result of blinker) {
            if(result.correct) {
                result.target.style.setProperty("border", "2px solid lime", "important");
                result.target.style.setProperty("background-color", "lime", "important");
            } else {
                result.target.style.setProperty("border", "2px solid pink", "important");
                result.target.style.setProperty("background-color", "pink", "important");
            }

            await new Promise((resolve, reject)=>{
                if (ConsentEngine.debugValues.clickDelay) {
                        setTimeout(()=>{
                        resolve();
                    }, 10);
                } else {
                    resolve();
                }
            });
        }
    }
}

class CssMatcher extends Matcher {
    constructor(config) {
        super(config);
    }

    matches() {
        let result = Tools.find(this.config);

        return result.target != null;
    }
}

class CheckboxMatcher extends Matcher {
    constructor(config) {
        super(config);
    }

    matches() {
        let result = Tools.find(this.config);
        
        if(result.target == null) {
            //No checkbox found, error
            throw new Error("No checkbox found, cannot check state");
        }

        if(this.config.negated) {
            return !result.target.checked;
        }

        return result.target.checked;
    }
}

class URLMatcher extends Matcher {
    constructor(config) {
        super(config);
    }

    debug() {
        //Overriden to disable
    }

    matches() {
        if (ConsentEngine.debugValues.debugRules) {
            console.log("URL Matcher:", ConsentEngine.topFrameUrl, this.config);
        }

        let urls = this.config.url;
        if(!Array.isArray(urls)) {
            urls = [urls];
        }

        let matched = false;
        
        if(this.config.regexp) {
            for(let url of urls) {
                let regexp = new RegExp(url);
                if(regexp.exec(ConsentEngine.topFrameUrl) !== null) {
                    if (ConsentEngine.debugValues.debugRules) {
                        console.log("Matched URL regexp:", url);
                    }
                    matched = true;
                    break;
                }
            }
        } else {
            for(let url of urls) {
                if(ConsentEngine.topFrameUrl.indexOf(url) > -1) {
                    if (ConsentEngine.debugValues.debugRules) {
                        console.log("Matched URL:", url);
                    }
                    matched = true;
                    break;
                }
            }
        }

        if(this.config.negated) {
            //If the matcher should be negated, negate matched
            matched = !matched;
        }

        if (ConsentEngine.debugValues.debugRules) {
            console.log("Did URLMatcher match (after negate):", matched);
        }

        return matched;
    }
}

class Detector {
    constructor(config) {
        this.config = config;

        this.presentMatchers = [];
        this.showingMatchers = [];

        if(!Array.isArray(this.config.presentMatcher)) {
            this.config.presentMatcher = [this.config.presentMatcher];
        }
        if(!Array.isArray(this.config.showingMatcher)) {
            this.config.showingMatcher = [this.config.showingMatcher];
        }

        this.config.presentMatcher.forEach((matcher)=>{
            if(matcher != null) {
                this.presentMatchers.push(Matcher.createMatcher(matcher));
            }
        });
        this.config.showingMatcher.forEach((matcher)=>{
            if(matcher != null) {
                this.showingMatchers.push(Matcher.createMatcher(matcher));
            }
        });
    }

    detect() {
        if(this.presentMatchers.length === 0) {
            return false;
        }

        return this.presentMatchers.every((matcher)=>{
            return matcher.matches();
        });
    }

    isShowing() {
        if(this.showingMatchers.length === 0) {
            return true;
        }

        return this.showingMatchers.every((matcher)=>{
            return matcher.matches();
        });
    }
}

class Consent {
    constructor(config, cmp) {
        this.config = config;
        this.cmp = cmp;

        if(this.config.toggleAction != null) {
            this.toggleAction = Action.createAction(this.config.toggleAction, cmp);
        }

        if(this.config.matcher != null) {
            this.enabledMatcher = Matcher.createMatcher(this.config.matcher);
        }

        if(this.config.falseAction != null) {
            this.falseAction = Action.createAction(this.config.falseAction, cmp);
        }

        if(this.config.trueAction != null) {
            this.trueAction = Action.createAction(this.config.trueAction, cmp);
        }
    }

    async toggle() {
        return await this.toggleAction.execute();
    }

    isEnabled() {
        return this.enabledMatcher.matches();
    }

    async setEnabled(enabled) {
        if(this.toggleAction != null) {
            if(this.enabledMatcher == null) {
                //Toggle is only supported with a matcher
                if (ConsentEngine.debugValues.debugLog) {
                    throw new Error("Toggle consent action, without a matcher: "+JSON.stringify(this.config));
                }
                return;
            }
            try {
                if(this.isEnabled() !== enabled) {
                    await this.toggle();
                }
            } catch(e) {
                if (ConsentEngine.debugValues.debugLog) {
                    console.error("Error toggling:", e, this.config);
                }
            }
        } else {
            let handled = false;

            //If we have OnOffMatcher, we can reduce clicks to only happen when state is wrong
            if(this.enabledMatcher != null && this.enabledMatcher instanceof OnOffMatcher) {
                try {
                    if(this.isEnabled() && !enabled) {
                        await this.falseAction.execute();
                    } else if(!this.isEnabled() && enabled) {
                        await this.trueAction.execute();
                    }
                    handled = true;
                } catch(e) {
                    if (ConsentEngine.debugValues.debugLog) {
                        console.error("Error pushing on/off:", e, this.config);
                    }
                }
            }

            if(!handled) {
                if(enabled) {
                    await this.trueAction.execute();
                } else {
                    await this.falseAction.execute();
                }
            }
        }

        if (ConsentEngine.debugValues.paintMatchers) {
            if(this.enabledMatcher != null) {
                //Debug if state is correct
                await this.enabledMatcher.debug(enabled);
            }
        }
    }

    get type() {
        return this.config.type;
    }
}

class Action {
    static createAction(config, cmp) {
        try {
            switch (config.type) {
                case "click": return new ClickAction(config, cmp);
                case "multiclick": return new MultiClickAction(config, cmp);
                case "list": return new ListAction(config, cmp);
                case "consent": return new ConsentAction(config, cmp);
                case "ifcss": return new IfCssAction(config, cmp);
                case "waitcss": return new WaitCssAction(config, cmp);
                case "foreach": return new ForEachAction(config, cmp);
                case "hide": return new HideAction(config, cmp);
                case "slide": return new SlideAction(config, cmp);
                case "close": return new CloseAction(config, cmp);
                case "wait": return new WaitAction(config, cmp);
                case "ifallowall": return new IfAllowAllAction(config, cmp);
                case "ifallownone": return new IfAllowNoneAction(config, cmp);
                case "runrooted": return new RunRootedAction(config, cmp);
                case "runmethod": return new RunMethodAction(config, cmp);
                default: throw "Unknown action type: " + config.type;
            }
        } catch (e) {
            if(ConsentEngine.debugValues.debugLog) {
                console.error(e);
            }
            return new NopAction(config, cmp);
        }
    }

    constructor(config) {
        const self = this;

        this.config = config;

        if(ConsentEngine.debugValues.debugLog) {
            //Override execute, with logging variant
            let realExecute = this.execute;

            this.execute = async function (param) {
                self.logStart(param);
                try {
                    await realExecute.call(self, param);
                } catch (e) {
                    console.error(e);
                }
                self.logEnd();
            }
        }
    }

    get timeout() {
        if (this.config.timeout != null) {
            return this.config.timeout;
        } else {
            if (ConsentEngine.debugValues.clickDelay) {
                return 150;
            } else if(ConsentEngine.singleton.pipEnabled) {
                // Reduce click wait based on number of clicks
                if (ConsentEngine.singleton.getClicksSoFar()>100) return 0;
                if (ConsentEngine.singleton.getClicksSoFar()>20) return 1;
                if (ConsentEngine.singleton.getClicksSoFar()>5) return 10;
                return 100;
            } else {
                return 0;
            }
        }
    }

    logStart(param) {
        if (ConsentEngine.debugValues.debugLog) {
            console.group(this.constructor.name + ":", this.config, param);
        }
    }

    logEnd() {
        if (ConsentEngine.debugValues.debugLog) {
            console.groupEnd();
        }
    }

    async execute(param) {
        console.log("Remember to overrride execute()", this.constructor.name);
    }

    async waitTimeout(timeout) {
        return new Promise((resolve) => {
            setTimeout(() => { resolve(); }, timeout);
        });
    }

    getNumSteps() {
        console.warn("Missing getNumSteps on: "+this.constructor.name);
        return 0;
    }
}

class ListAction extends Action {
    constructor(config, cmp) {
        super(config);

        this.actions = [];
        config.actions.forEach((actionConfig) => {
            this.actions.push(Action.createAction(actionConfig, cmp));
        });
    }

    async execute(param) {
        for (let action of this.actions) {
            await action.execute(param);
        }
    }

    getNumSteps() {
        let steps = 0;

        this.actions.forEach((action)=>{
            steps += action.getNumSteps();
        });

        return steps;
    }
}

class CloseAction extends Action {
    constructor(config, cmp) {
        super(config);
    }

    async execute(param) {
        window.close();
        return 1; // Closing window counts as a click
    }

    getNumSteps() {
        return 1;
    }
}

class WaitAction extends Action {
    constructor(config, cmp) {
        super(config);
    }

    async execute(param) {
        let self = this;
        await new Promise((resolve, reject) => {
            setTimeout(() => { resolve() }, self.config.waitTime);
        });
    }

    getNumSteps() {
        return 1;
    }
}

class ClickAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;
    }

    async execute(param) {
        let result = Tools.find(this.config);

        if (result.target != null) {
            let pipScroll = false;
            if(ConsentEngine.singleton.pipEnabled) {
                pipScroll = result.target.closest(".ConsentOMatic-CMP-PIP") != null;
            }
    
            let isShowing = result.target.offsetHeight !== 0;

            if (isShowing && (ConsentEngine.debugValues.clickDelay || pipScroll)) {
                result.target.scrollIntoView({
                    behavior: "smooth",
                    block: "center",
                    inline: "center"
                });
            }

            if(isShowing && this.config.noTimeout !== true) {
                await this.waitTimeout(this.timeout);
            }

            if (ConsentEngine.debugValues.debugClicks) {
                console.log("Clicking: [openInTab: " + this.config.openInTab + "]", result.target);
            }

            if (ConsentEngine.debugValues.clickDelay || pipScroll) {
                result.target.focus({ preventScroll: true });
            }

            if (this.config.openInTab) {
                //Handle osx behaving differently?
                result.target.dispatchEvent(new MouseEvent("click", { ctrlKey: true, shiftKey: true }));
            } else {
                result.target.click();
            }

            ConsentEngine.singleton.registerClick();

            if(isShowing && this.config.noTimeout !== true) {
                await this.waitTimeout(this.timeout);
            }
        }
    }

    getNumSteps() {
        return 1;
    }
}

class MultiClickAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;
    }

    async execute(param) {
        let results = Tools.find(this.config, true);
        if (ConsentEngine.debugValues.debugClicks) {
            console.log("MultiClicking:", results);
        }
	results.forEach(result=>{
	    if (result.target!=null){
		result.target.click();
	    }
	});
        
        // TODO: Register more clicks simultaneously
	results.forEach(result=>{
	    if (result.target!=null){
    		ConsentEngine.singleton.registerClick();
    	    }
    	});
        
        // TODO: Consider how this should be shown in PiP?
    }

    getNumSteps() {
        return 50; // STUB: Poor estimate
    }
}

class ConsentAction extends Action {
    constructor(config, cmp) {
        super(config);

        let self = this;

        this.consents = [];

        this.config.consents.forEach((consentConfig) => {
            self.consents.push(new Consent(consentConfig, cmp));
        });
    }

    async execute(consentTypes) {
        for (let consent of this.consents) {
            let shouldBeEnabled = false;

            if (consentTypes.hasOwnProperty(consent.type)) {
                shouldBeEnabled = consentTypes[consent.type];
            }

            await consent.setEnabled(shouldBeEnabled);
        }
    }

    getNumSteps() {
        return 1;
    }
}

class IfCssAction extends Action {
    constructor(config, cmp) {
        super(config);

        if (config.trueAction != null) {
            this.trueAction = Action.createAction(config.trueAction, cmp);
        }

        if (config.falseAction != null) {
            this.falseAction = Action.createAction(config.falseAction, cmp);
        }
    }

    async execute(param) {
        let result = Tools.find(this.config);

        if (result.target != null) {
            if (this.trueAction != null) {
                await this.trueAction.execute(param);
            }
        } else {
            if (this.falseAction != null) {
                await this.falseAction.execute(param);
            }
        }
    }

    getNumSteps() {
        let steps = 0;

        if(this.trueAction != null) {
            steps += this.trueAction.getNumSteps();
        }

        if(this.falseAction != null) {
            steps += this.falseAction.getNumSteps();
        }

        return Math.round(steps / 2);
    }
}

class WaitCssAction extends Action {
    constructor(config, cmp) {
        super(config);
    }

    async execute(param) {
        let self = this;
        let negated = false;

        if (self.config.negated) {
            negated = self.config.negated;
        }

        if (ConsentEngine.debugValues.debugLog) {
            console.time("Waiting [" + negated + "]:" + this.config.target.selector);
        }

        await new Promise((resolve) => {
            let numRetries = 10;
            let waitTime = 250;

            if (self.config.retries) {
                numRetries = self.config.retries;
            }

            if (self.config.waitTime) {
                waitTime = self.config.waitTime;
            }

            function checkCss() {
                let result = Tools.find(self.config);

                if (negated) {
                    if (result.target != null) {
                        if (numRetries > 0) {
                            numRetries--;
                            setTimeout(checkCss, waitTime);
                        } else {
                            if (ConsentEngine.debugValues.debugLog) {
                                console.timeEnd("Waiting [" + negated + "]:" + self.config.target.selector);
                            }
                            resolve();
                        }
                    } else {
                        if (ConsentEngine.debugValues.debugLog) {
                            console.timeEnd("Waiting [" + negated + "]:" + self.config.target.selector);
                        }
                        resolve();
                    }
                } else {
                    if (result.target != null) {
                        if (ConsentEngine.debugValues.debugLog) {
                            console.timeEnd("Waiting [" + negated + "]:" + self.config.target.selector);
                        }
                        resolve();
                    } else {
                        if (numRetries > 0) {
                            numRetries--;
                            setTimeout(checkCss, waitTime);
                        } else {
                            if (ConsentEngine.debugValues.debugLog) {
                                console.timeEnd("Waiting [" + negated + "]:" + self.config.target.selector);
                            }
                            resolve();
                        }
                    }
                }
            }

            checkCss();
        });
    }

    getNumSteps() {
        return 1;
    }
}

class NopAction extends Action {
    constructor(config, cmp) {
        super(config);
    }

    async execute(param) {
        //NOP
    }

    getNumSteps() {
        return 0;
    }
}

class ForEachAction extends Action {
    constructor(config, cmp) {
        super(config);

        if(this.config.action != null) {
            this.action = Action.createAction(this.config.action, cmp);
        } else {
            console.warn("Missing action on ForEach: ", this);
        }
    }

    async execute(param) {
        if(this.action != null) {
            let results = Tools.find(this.config, true);
            let oldBase = Tools.base;

            for (let result of results) {
                if (result.target != null) {
                    Tools.setBase(result.target);
                    await this.action.execute(param);
                }
            }

            Tools.setBase(oldBase);
        }
    }

    getNumSteps() {
        if(this.action != null) {
            return this.action.getNumSteps();
        }

        return 0;
    }
}

class HideAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;
    }

    async execute(param) {
        if(ConsentEngine.debugValues.skipHideMethod) {
            return;
        }

        let self = this;
        let result = Tools.find(this.config);

        if (result.target != null) {
            this.cmp.hiddenTargets.push(result.target);

            if(this.config.hideFromDetection === true) {
                result.target.classList.add("ConsentOMatic-CMP-NoDetect");
            }

            if(ConsentEngine.generalSettings.hideInsteadOfPIP || this.config.forceHide === true) {
                result.target.classList.add("ConsentOMatic-CMP-Hider");
            } else {
                ConsentEngine.singleton.enablePip();
                result.target.classList.add("ConsentOMatic-CMP-PIP");
                
                if(typeof result.target.savedStyles === "undefined") {
                    result.target.savedStyles = result.target.getAttribute("style");
                }

                function setStyles() {
                    if(!result.target.matches(".ConsentOMatic-CMP-PIP")) {
                        return;
                    }

                    let preview = document.querySelector(".ConsentOMatic-Progres-Preview");
                    let scale = 0.25;
                    if(preview != null) {
                        let width = preview.offsetWidth - 4;
                        let height = preview.offsetHeight - 4;
    
                        let targetWidth = result.target.offsetWidth;
                        let targetHeight = result.target.offsetHeight;
    
                        if(result.target.offsetHeight === 0) {
                            result.target.style.setProperty("height", "100%", "important");
                            targetHeight = result.target.offsetHeight;
                        }

                        let widthScale = width / targetWidth;
                        let heightScale = height / targetHeight;
    
                        scale = Math.min(widthScale, heightScale);
                    }
    
                    result.target.style.setProperty("position", "fixed", "important");
                    result.target.style.setProperty("left", "initial","important");
                    result.target.style.setProperty("top","initial","important");
                    result.target.style.setProperty("right",  "0px", "important");
                    result.target.style.setProperty("bottom", "2px", "important");
                    result.target.style.setProperty("transform", "translateY(-4rem) scale("+scale+")", "important");
                    result.target.style.setProperty("transform-origin", "right bottom", "important");
                    result.target.style.setProperty("transition", "transform 0.15s ease-in-out", "important");
                    result.target.style.setProperty("contain", "paint", "important");
                    result.target.style.setProperty("border", "none", "important");
                    result.target.style.setProperty("box-shadow", "none", "important");
                    result.target.style.setProperty("z-index", "2147483647", "important");
                    result.target.style.setProperty("grid-column", "none", "important");
                    result.target.style.setProperty("grid-row", "none", "important");
                }

                setStyles();

                await new Promise((resolve)=>{
                    requestAnimationFrame(()=>{
                        resolve();
                    });
                });

                let entriesSeen = new Set();

                let observer = new ResizeObserver((entries)=>{
                    for(let entry of entries) {
                        if(!entriesSeen.has(entry.target)) {
                            entriesSeen.add(entry.target);
                        } else {
                            setStyles();
                        }
                    }
                });

                startObserver();

                function startObserver() {
                    entriesSeen.clear();
                    observer.observe(result.target);
                }

                this.cmp.observers.push(observer);

                let observer2 = new MutationObserver((mutations)=>{
                    setStyles();
                });

                this.cmp.observers.push(observer2);
                observer2.observe(result.target, {
                    attributes: true,
                    attributeFilter: ["style"]
                });
            }
        }
    }

    getNumSteps() {
        return 1;
    }
}

class SlideAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;
    }

    async execute(param) {
        let result = Tools.find(this.config);

        let dragResult = Tools.find(this.config.dragTarget);

        if (result.target != null) {
            let targetBounds = result.target.getBoundingClientRect();
            let dragTargetBounds = dragResult.target.getBoundingClientRect();

            let yDiff = dragTargetBounds.top - targetBounds.top;
            let xDiff = dragTargetBounds.left - targetBounds.left;

            if (this.config.axis.toLowerCase() === "y") {
                xDiff = 0;
            }
            if (this.config.axis.toLowerCase() === "x") {
                yDiff = 0;
            }

            let screenX = window.screenX + targetBounds.left + targetBounds.width / 2.0;
            let screenY = window.screenY + targetBounds.top + targetBounds.height / 2.0;
            let clientX = targetBounds.left + targetBounds.width / 2.0;
            let clientY = targetBounds.top + targetBounds.height / 2.0;

            let mouseDown = document.createEvent("MouseEvents");
            mouseDown.initMouseEvent(
                "mousedown",
                true,
                true,
                window,
                0,
                screenX,
                screenY,
                clientX,
                clientY,
                false,
                false,
                false,
                false,
                0,
                result.target
            );

            let mouseMove = document.createEvent("MouseEvents");
            mouseMove.initMouseEvent(
                "mousemove",
                true,
                true,
                window,
                0,
                screenX + xDiff,
                screenY + yDiff,
                clientX + xDiff,
                clientY + yDiff,
                false,
                false,
                false,
                false,
                0,
                result.target
            );

            let mouseUp = document.createEvent("MouseEvents");
            mouseUp.initMouseEvent(
                "mouseup",
                true,
                true,
                window,
                0,
                screenX + xDiff,
                screenY + yDiff,
                clientX + xDiff,
                clientY + yDiff,
                false,
                false,
                false,
                false,
                0,
                result.target
            );

            result.target.dispatchEvent(mouseDown);
            await this.waitTimeout(10);
            result.target.dispatchEvent(mouseMove);
            await this.waitTimeout(10);
            result.target.dispatchEvent(mouseUp);
            ConsentEngine.singleton.registerClick();
        }
    }

    getNumSteps() {
        return 1;
    }
}

class IfAllowAllAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;
 
        if (config.trueAction != null) {
            this.trueAction = Action.createAction(config.trueAction, cmp);
        }

        if (config.falseAction != null) {
            this.falseAction = Action.createAction(config.falseAction, cmp);
        }
    }

    async execute(consentTypes) {
        let allTrue = true;
        Object.keys(consentTypes).forEach((key)=>{
            let value = consentTypes[key];

            if(value === false) {
                allTrue = false;
            }
        });

        if (allTrue) {
            if (this.trueAction != null) {
                await this.trueAction.execute(consentTypes);
            }
        } else {
            if (this.falseAction != null) {
                await this.falseAction.execute(consentTypes);
            }
        }
    }

    getNumSteps() {
        let steps = 0;

        if(this.trueAction != null) {
            steps += this.trueAction.getNumSteps();
        }

        if(this.falseAction != null) {
            steps += this.falseAction.getNumSteps();
        }

        return Math.round(steps / 2);
    }
}

class IfAllowNoneAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;
 
        if (config.trueAction != null) {
            this.trueAction = Action.createAction(config.trueAction, cmp);
        }

        if (config.falseAction != null) {
            this.falseAction = Action.createAction(config.falseAction, cmp);
        }
    }

    async execute(consentTypes) {
        let allFalse = true;

        Object.keys(consentTypes).forEach((key)=>{
            let value = consentTypes[key];

            if(value === true) {
                allFalse = false;
            }
        });

        if (allFalse) {
            if (this.trueAction != null) {
                await this.trueAction.execute(consentTypes);
            }
        } else {
            if (this.falseAction != null) {
                await this.falseAction.execute(consentTypes);
            }
        }
    }

    getNumSteps() {
        let steps = 0;

        if(this.trueAction != null) {
            steps += this.trueAction.getNumSteps();
        }

        if(this.falseAction != null) {
            steps += this.falseAction.getNumSteps();
        }

        return Math.round(steps / 2);
    }
}

class RunRootedAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;

        if(this.config.action != null) {
            this.action = Action.createAction(this.config.action, cmp);
        } else {
            console.warn("Missing action on RunRooted: ", this);
        }
    }

    async execute(params) {
        if(this.config.action != null) {

            //Save root
            let oldRoot = Tools.getBase();

            //Reset to null root
            if(this.config.ignoreOldRoot === true) {
                Tools.setBase(null);
            }

            //Find new root
            let result = Tools.find(this.config);

            if(result.target != null) {
                //Set new root
                Tools.setBase(result.target);
                
                //RUN
                await this.action.execute(params);
            }

            //Set old base back
            Tools.setBase(oldRoot);
        }
    }

    getNumSteps() {
        if(this.action != null) {
            return this.action.getNumSteps();
        }

        return 0;
    }
}

class RunMethodAction extends Action {
    constructor(config, cmp) {
        super(config);
        this.cmp = cmp;
    }

    async execute(params) {
        if(this.config.method == null) {
            console.warn("Missing option 'method' on RunMethodAction");
            return;
        }

        let methodName = this.config.method.toUpperCase();

        if(!this.cmp.hasMethod(methodName)) {
            console.warn("CMP does not have method ["+methodName+"]", this.cmp);
            return;
        }

        if(!this.cmp.isCustomMethod(methodName)) {
            console.warn("CMP method ["+methodName+"] is not a custom method!");
            return;
        }

        await this.cmp.runMethod(methodName, params);
    }

    getNumSteps() {
        if(this.cmp.hasMethod(this.config.method)) {
            return this.cmp.getNumStepsForMethod(this.config.method);
        }

        return 0;
    }
}
class CMP {
    constructor(name, config) {
        let self = this;

        this.name = name;

        this.detectors = [];
        config.detectors.forEach((detectorConfig)=>{
            self.detectors.push(new Detector(detectorConfig));
        });

        this.methods = new Map();
        config.methods.forEach((methodConfig)=>{
            if(methodConfig.action != null) {
                let action = Action.createAction(methodConfig.action, this);
                action.customMethod = methodConfig.custom;
                self.methods.set(methodConfig.name, action);
            }
        });

        this.hiddenTargets = [];
        this.observers = [];
    }

    stopObservers() {
        this.observers.forEach((observer)=>{
            observer.disconnect();
        });
    }

    unHideAll() {
        let ourStyles = [
            "position",
            "left",
            "top",
            "right",
            "bottom",
            "transform",
            "transform-origin",
            "transition",
            "contain",
            "border",
            "box-shadow",
            "z-index",
            "animation",
            "grid-column",
            "grid-row"
        ];

        this.hiddenTargets.forEach((target)=>{
            target.classList.remove("ConsentOMatic-CMP-Hider");
            target.classList.remove("ConsentOMatic-CMP-PIP");

            //Find styles we have not set

            if(typeof target.savedStyles !== "undefined") {
                let styleAttrSplit = target.getAttribute("style").split(";").filter((style)=>{return style.trim().length > 0});

                let notOurStyles = [];

                styleAttrSplit.forEach((style)=>{
                    style = style.trim();
                    if(style.length > 0 && style.indexOf(":") !== -1) {
                        let styleSplit = style.split(":");
                        let name = styleSplit[0].trim();
                        let value = styleSplit[1].trim();

                        if(!ourStyles.includes(name)) {
                            notOurStyles.push({name, value});
                        }
                    }
                });

                //Reload saved styles
                if(target.savedStyles === null) {
                    target.removeAttribute("style");
                }
                target.setAttribute("style", target.savedStyles);

                //Set styles again, we did not set
                notOurStyles.forEach(({name, value})=>{
                    target.style.setProperty(name, value);
                });
            }
        });
    }

    detect() {
        if(ConsentEngine.debugValues.debugRules) {
            console.groupCollapsed("Testing:", this.name);
        }
        try {
            let detector = this.detectors.find((detector)=>{
                return detector.detect();
            });

            if(detector != null && ConsentEngine.debugValues.debugRules) {
                console.log("Triggered detector: ", detector);
            }

            if(ConsentEngine.debugValues.debugRules) {
                console.groupEnd();
            }
    
            return detector != null;
        } catch(e) {
            if(ConsentEngine.debugValues.debugRules) {
                console.warn(e);
                console.groupEnd();
            }
        }

        return false;
    }

    isShowing() {
        let detector = this.detectors.find((detector)=>{
            return detector.detect();
        });

        return detector.isShowing();
    }

    isUtility() {
        return this.methods.has("UTILITY") && this.methods.size === 1;
    }

    hasMethod(name) {
        return this.methods.has(name);
    }

    isCustomMethod(name) {
        let action = this.methods.get(name);

        if(action != null) {
            return action.customMethod === true;
        }

        return false;
    }

    async runMethod(name, param = null) {
        let action = this.methods.get(name);

        if(action != null) {
            if(!this.isCustomMethod(name)) {
                ConsentEngine.singleton.currentMethodStepsTotal = this.getNumStepsForMethod(name);
            }

            if(ConsentEngine.debugValues.debugLog) {
                console.log("Triggering method: ", name);
            }
            await action.execute(param);

            if(!this.isCustomMethod(name)) {
                ConsentEngine.singleton.currentMethodDone();
            }


        } else {
            //Make no method behave as if an action was called, IE. push os back on the task stack
            await new Promise((resolve)=>{
                setTimeout(()=>{
                    resolve();
                }, 0);
            });
        }
    }

    getNumStepsForMethod(method) {
        let action = this.methods.get(method);

        if(action != null) {
            return action.getNumSteps();
        }

        return 0;
    }

    getNumSteps() {
        let totalSteps = 0;

        this.methods.forEach((action, name)=>{
            if(name !== "UTILITY") {
                let steps = action.getNumSteps();

                if(name === "HIDE_CMP") {
                    //Hide CMP is called twice
                    totalSteps += (2 * steps);
                } else {
                    totalSteps += steps;
                }
            }
        });

        return totalSteps;
    }
}

class ConsentEngine {
    constructor(config, consentTypes, handledCallback) {
        let self = this;

        this.consentTypes = consentTypes;

        this.cmps = [];

        this.handledCallback = handledCallback;

        this.triedCMPs = new Set();

        this.numClicks = 0;
        this.pipEnabled = false;

        Object.keys(config).forEach((key) => {
            try {
                self.cmps.push(new CMP(key, config[key]));
            } catch (err) {
                console.groupCollapsed("Invalid CMP (" + key + ") detected, please update GDPR consent engine or fix the rule generating this error:");
                console.error(err);
                console.groupEnd();
            }
        });

        this.cmps.sort((cmp1, cmp2) => {
            if (cmp1.isUtility()) {
                return -1;
            } else if (cmp2.isUtility()) {
                return 1;
            } else {
                return 0;
            }
        })

        this.setupObserver();
        this.startObserver();

        this.startStopTimeout();

        window.addEventListener("DOMContentLoaded", ()=>{
            self.handleMutations([]);
        });
        this.domScannerIntervalID = setInterval(()=>{
            self.handleMutations([]);
        }, 500);
    }


    /**
     * Generates or removes a temporary custom stylesheet that enforces the currently stored scroll behaviours
     */
    static enforceScrollBehaviours(shouldEnforce) {
        //Make sure we enforce on parent if inside iframe
        let insideIframe = window !== window.parent;
    
        // Iframe scroll-behaviour enforcement stripped — requires chrome.runtime
        // messaging which is unavailable outside extension context. Non-critical:
        // only affects cosmetic scroll restoration in cross-origin iframes.
    
        let stylesheetElement = document.querySelector("#consent-scrollbehaviour-override");
        if (stylesheetElement) {
            stylesheetElement.textContent = "";
        }

        document.querySelector("html").classList.remove("consent-scrollbehaviour-override");
        document.querySelector("body").classList.remove("consent-scrollbehaviour-override");

        if (!shouldEnforce) return;

        document.querySelector("html").classList.add("consent-scrollbehaviour-override");
        document.querySelector("body").classList.add("consent-scrollbehaviour-override");

        if (!stylesheetElement) {
            stylesheetElement = document.createElement("style");
            stylesheetElement.id = "consent-scrollbehaviour-override";
            document.documentElement.appendChild(stylesheetElement);
        }
        let content = Object.entries(window.consentScrollBehaviours).map(entry => {
            const [element, rules] = entry;
            return element + "{" + rules.map(rule => {
                let hasImportant = rule.value.includes("important");
                return rule.property + ":" + rule.value + (hasImportant ? "" : "!important");
            }).join(";") + "}";
        }).join("");
        if (ConsentEngine.debugValues.debugLog) {
            console.log(content);
        }
        stylesheetElement.textContent = content;
    }


    enablePip() {
        this.pipEnabled = true;
        if (this.modal != null) {
            this.modal.classList.add("ConsentOMatic-PIP")
        }
        ConsentEngine.enforceScrollBehaviours(true);
    }

    registerClick() {
        let clickFraction = 1 / this.currentMethodStepsTotal;

        this.numClicks++;

        if (this.currentMethodFraction == 0) {
            this.currentMethodFraction = clickFraction;
        } else {
            let rest = 1 - this.currentMethodFraction;

            this.currentMethodFraction += rest * (clickFraction / 2.0);
        }

        this.calculateProgress();
    }

    getClicksSoFar() {
        return this.numClicks;
    }

    currentMethodDone() {
        this.completedSteps += this.currentMethodStepsTotal;
        this.currentMethodFraction = 1;
        this.calculateProgress();
        this.currentMethodStepsTotal = 0;
        this.currentMethodFraction = 0;
    }

    startStopTimeout() {
        const self = this;

        if (this.stopEngineId != null) {
            clearTimeout(this.stopEngineId);
        }

        this.stopEngineId = setTimeout(() => {
            if (ConsentEngine.debugValues.debugLog) {
                console.log("No CMP detected in 5 seconds, stopping engine...");
            }

            if(self.queueId != null) {
                clearTimeout(self.queueId);
            }

            clearInterval(self.domScannerIntervalID);

            self.handledCallback({
                handled: false
            });
            this.stopObserver();
        }, 5000);
    }

    async handleMutations(mutations) {
        const self = this;

        if (this.queueId == null && !self.checkRunning) {
            this.queueId = setTimeout(async () => {
                self.queueId = null;
                try {
                    self.checkForCMPs();
                } catch (e) {
                    console.error(e);
                }
            }, 250);
        }
    }

    async checkForCMPs() {
        const self = this;

        self.checkRunning = true;

        //Wait for a super small while, to make async
        await new Promise((resolve)=>{
            setTimeout(()=>{
                resolve();
            }, 0);
        });

        if (ConsentEngine.debugValues.debugLog) {
            console.groupCollapsed("findCMP");
        }
        let cmps = this.findCMP();
        if (ConsentEngine.debugValues.debugLog) {
            console.groupEnd();
        }

        cmps = cmps.filter((cmp) => {
            return !self.triedCMPs.has(cmp.name);
        });

        if (cmps.length > 0) {
            this.stopObserver();

            if (cmps.length > 1) {
                console.warn("Found multiple CMPS's maybee rewise detection rules...", cmps);
            }

            let cmp = cmps[0];

            if (ConsentEngine.debugValues.debugLog) {
                console.log("CMP Detected: ", cmp.name);
                console.groupCollapsed(cmp.name + " - isShowing?");
            }

            this.triedCMPs.add(cmp.name);

            //Check if popup shows, then do consent stuff
            let numberOfTries = 5;
            async function checkIsShowing() {
                if (cmp.isShowing()) {
                    self.currentCMP = cmp;
                    if (ConsentEngine.debugValues.debugLog) {
                        console.groupEnd();
                        console.log(cmp.name + " - Showing");
                    }
                    setTimeout(async () => {
                        if (cmp.isUtility()) {
                            if (ConsentEngine.debugValues.debugLog) {
                                console.groupCollapsed(cmp.name + " - UTILITY");
                            }
                            await cmp.runMethod("UTILITY", self.consentTypes);
                            if (ConsentEngine.debugValues.debugLog) {
                                console.groupEnd();
                            }
                            if (!ConsentEngine.debugValues.skipHideMethod) {
                                if (!ConsentEngine.debugValues.dontHideProgressDialog) {
                                    cmp.stopObservers();
                                }
                            }

                            self.checkRunning = false;

                            self.startObserver();
                            self.handleMutations([]);
                        } else {
                            try {
                                self.numClicks = 0;

                                if (!ConsentEngine.debugValues.skipHideMethod) {
                                    self.showProgressDialog("Handling " + cmp.name + "...");
                                }

                                self.totalSteps = cmp.getNumSteps();
                                self.completedSteps = 0;
                                self.currentMethodStepsTotal = 0;
                                self.currentMethodFraction = 0;
                                self.updateProgress(0);

                                if (!ConsentEngine.debugValues.skipHideMethod) {
                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.groupCollapsed(cmp.name + " - HIDE_CMP");
                                    }
                                    await cmp.runMethod("HIDE_CMP", self.consentTypes);

                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.groupEnd();
                                    }
                                }
                                if(!ConsentEngine.debugValues.skipOpenMethod) {
                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.groupCollapsed(cmp.name + " - OPEN_OPTIONS");
                                    }
                                }
                                await cmp.runMethod("OPEN_OPTIONS", self.consentTypes);
                                if (ConsentEngine.debugValues.debugLog) {
                                    console.groupEnd();
                                }

                                if (!ConsentEngine.debugValues.skipHideMethod) {
                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.groupCollapsed(cmp.name + " - HIDE_CMP");
                                    }
                                    await cmp.runMethod("HIDE_CMP", self.consentTypes);
                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.groupEnd();
                                    }
                                }

                                if (ConsentEngine.debugValues.debugLog) {
                                    console.groupCollapsed(cmp.name + " - DO_CONSENT");
                                }
                                await cmp.runMethod("DO_CONSENT", self.consentTypes);
                                if (ConsentEngine.debugValues.debugLog) {
                                    console.groupEnd();
                                }

                                if (!ConsentEngine.debugValues.skipSubmit) {
                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.groupCollapsed(cmp.name + " - SAVE_CONSENT");
                                    }
                                    await cmp.runMethod("SAVE_CONSENT", self.consentTypes);
                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.groupEnd();
                                    }
                                }
                                if (!(self.numClicks > 0)) {
                                    if (ConsentEngine.debugValues.debugLog) {
                                        console.log("Consent-O-Matic click count was 0 for CMP:", self.numClicks, cmp.name);
                                    }
                                    self.numClicks = 0; // Catch-all for NaN, negative numbers etc.
                                }

                                self.updateProgress(1.0);

                                self.handledCallback({
                                    handled: true,
                                    cmpName: cmp.name,
                                    clicks: self.numClicks
                                });
                            } catch (e) {
                                console.log("Error during consent handling:", e);
                                self.handledCallback({
                                    handled: false,
                                    error: true
                                });
                            }
                            if (!ConsentEngine.debugValues.skipHideMethod) {
                                if (!ConsentEngine.debugValues.dontHideProgressDialog) {
                                    self.currentCMP = null;
                                    self.hideProgressDialog();
                                    cmp.stopObservers();
                                    cmp.unHideAll();
                                }
                            }
                            clearTimeout(self.stopEngineId);
                            clearInterval(self.domScannerIntervalID);

                            self.checkRunning = false;

                            // Remove focus from anything inside the closed popups
                            window.focus();
                            if (document.activeElement) document.activeElement.blur();
                            document.body.focus();
                        }
                    }, 0);
                } else {
                    if (numberOfTries > 0) {
                        numberOfTries--;
                        setTimeout(checkIsShowing, 250);
                    } else {
                        if (ConsentEngine.debugValues.debugLog) {
                            console.groupEnd();
                            console.log(cmp.name + " - Not showing");
                        }
                        self.checkRunning = false;
                        self.startObserver();
                        self.startStopTimeout()
                        self.handleMutations([]);
                    }
                }
            }

            await checkIsShowing();
        } else {
            self.checkRunning = false;
        }
    }

    unHideAll() {
        if (this.currentCMP != null) {
            this.currentCMP.stopObservers();
            this.currentCMP.unHideAll();
        }
    }

    showProgressDialog(text) {
        const self = this;

        if (ConsentEngine.debugValues.debugLog) {
            console.log("Showing progress...");
        }
        if (this.dialogTimeoutId != null) {
            clearTimeout(this.dialogTimeoutId);
            this.dialogTimeoutId = null;
            if (this.dialog != null) {
                this.dialog.remove();
                this.dialog = null;
            }
            if (this.modal != null) {
                this.modal.remove();
                this.modal = null;
            }
        }

        this.modal = document.createElement("div");
        this.modal.classList.add("ConsentOMatic-Progress-Dialog-Modal");
        this.dialog = document.createElement("div");
        this.dialog.classList.add("ConsentOMatic-Progress-Dialog");

        this.preview = document.createElement("div");
        this.preview.classList.add("ConsentOMatic-Progres-Preview");
        this.modal.appendChild(this.preview);

        let contents = document.createElement("p");
        contents.innerText = text;
        this.dialog.appendChild(contents);
        document.body.appendChild(this.modal);
        this.modal.appendChild(this.dialog);

        this.dialog.addEventListener("click", () => {
            self.unHideAll();
        });

        setTimeout(() => {
            this.modal.classList.add("ConsentOMatic-Progress-Started");
        }, 0);
    }

    calculateProgress() {
        let fraction = (this.completedSteps + this.currentMethodStepsTotal * this.currentMethodFraction) / this.totalSteps;
        this.updateProgress(fraction);
    }

    updateProgress(fraction) {
        if (this.modal != null) {
            this.modal.style.setProperty("--progress", fraction);
        }
    }

    hideProgressDialog() {
        let self = this;
        if (ConsentEngine.debugValues.debugLog) {
            console.log("Hiding progress...");
        }
        this.modal.classList.add("ConsentOMatic-Progress-Complete");
        this.dialogTimeoutId = setTimeout(() => {
            self.modal.remove();
            self.dialog.remove();
            self.dialog = null;
        }, 1000);
        ConsentEngine.enforceScrollBehaviours(false);
    }

    setupObserver() {
        let self = this;

        this.observer = new MutationObserver((mutations) => {
            this.handleMutations(mutations);
        });
    }

    startObserver() {
        if (document.body != null) {
            this.observer.observe(document.body, {
                childList: true,
                attributes: true,
                subtree: true
            });
        }
    }

    stopObserver() {
        this.observer.disconnect();
    }

    findCMP() {
        return this.cmps.filter((cmp) => {
            return cmp.detect();
        });
    }
}

ConsentEngine.singleton = null;